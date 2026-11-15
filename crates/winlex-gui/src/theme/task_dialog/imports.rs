// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10. Rust integration: Winlex contributors.
//! Bounded PE import discovery and transactional comctl32-only pointer replacement.
//! Forward to resolved exports, never a delay-load stub that could undo the patch.
use crate::native::*;
use std::{
    mem, ptr,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::core::HRESULT;

pub(super) type ColorFn =
    unsafe extern "system" fn(HTHEME, i32, i32, i32, *mut COLORREF) -> HRESULT;
pub(super) type BackgroundFn =
    unsafe extern "system" fn(HTHEME, HDC, i32, i32, *const RECT, *const DTBGOPTS) -> HRESULT;

// Forwarding pointers can still be used by an in-flight callback after the last
// slot has been restored. Keep one uxtheme loader reference for process lifetime.
// Do not rely on a borrowed GetModuleHandle result or a delay-loader reference.
static EXPORTS: OnceLock<Option<(usize, usize)>> = OnceLock::new();
fn exports() -> Option<(usize, usize)> {
    *EXPORTS.get_or_init(|| unsafe {
        let module = LoadLibraryW(w!("uxtheme.dll"));
        if module.is_null() {
            return None;
        }
        let color = GetProcAddress(module, c"GetThemeColor".as_ptr().cast());
        let background = GetProcAddress(module, c"DrawThemeBackgroundEx".as_ptr().cast());
        match (color, background) {
            (Some(color), Some(background)) => Some((color as usize, background as usize)),
            _ => {
                FreeLibrary(module);
                None
            }
        }
    })
}

pub(super) unsafe fn original_color() -> ColorFn {
    unsafe { mem::transmute::<usize, ColorFn>(exports().expect("installed theme hook exports").0) }
}
pub(super) unsafe fn original_background() -> BackgroundFn {
    unsafe {
        mem::transmute::<usize, BackgroundFn>(exports().expect("installed theme hook exports").1)
    }
}

struct Patch {
    slot: usize,
    before: usize,
    after: usize,
    protection: Option<u32>,
}
unsafe fn protect_slot(pointer: *mut usize, protection: u32, old: &mut u32) -> bool {
    unsafe { VirtualProtect(pointer.cast(), mem::size_of::<usize>(), protection, old) != 0 }
}
impl Patch {
    unsafe fn replace(&mut self, install: bool) -> bool {
        unsafe {
            let pointer = self.slot as *mut usize;
            let mut old = 0;
            if !protect_slot(pointer, PAGE_READWRITE, &mut old) {
                return false;
            }
            let protection = self.protection.unwrap_or(old);
            self.protection = Some(protection);
            let (expected, value) = if install {
                (self.before, self.after)
            } else {
                (self.after, self.before)
            };
            let atom = AtomicUsize::from_ptr(pointer);
            // Do not overwrite a subsequent third-party hook.
            let changed = atom
                .compare_exchange(expected, value, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            let already_restored = !install && atom.load(Ordering::Acquire) != self.after;
            let mut ignored = 0;
            if protect_slot(pointer, protection, &mut ignored) {
                self.protection = None;
            }
            (changed || already_restored) && self.protection.is_none()
        }
    }
}

pub(super) struct Hooks {
    references: usize,
    module: usize,
    address: usize,
    patches: Vec<Patch>,
}
impl Hooks {
    pub(super) const fn new() -> Self {
        Self {
            references: 0,
            module: 0,
            address: 0,
            patches: Vec::new(),
        }
    }
    pub(super) fn acquire(&mut self, address: usize, color: usize, background: usize) -> bool {
        unsafe {
            if self.references != 0 {
                if self.address != address {
                    return false;
                }
                self.references += 1;
                return true;
            }
            if !self.restore() || address == 0 {
                return false;
            }
            let mut module = ptr::null_mut();
            // Reference the activation-context-selected v6 module, not name-only v5.
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                address as *const u16,
                &mut module,
            ) == 0
            {
                return false;
            }
            self.module = module as usize;
            let mut path = [0u16; 32768];
            let count = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
            let name = &path[..count.min(path.len())];
            let basename = name
                .rsplit(|ch| *ch == b'\\' as u16 || *ch == b'/' as u16)
                .next()
                .unwrap_or(name);
            let expected = wide("comctl32.dll");
            if basename.len() != expected.len() - 1
                || !basename
                    .iter()
                    .zip(&expected)
                    .all(|(a, b)| ascii_lower(*a) == ascii_lower(*b))
            {
                self.restore();
                return false;
            }
            let Some(image) = image(module) else {
                self.restore();
                return false;
            };
            let Some(slots) = find_slots(image, mem::size_of::<usize>()) else {
                self.restore();
                return false;
            };
            if !slots.iter().any(|s| s.1 == Function::Color)
                || !slots.iter().any(|s| s.1 == Function::Background)
            {
                self.restore();
                return false;
            }
            if exports().is_none() {
                self.restore();
                return false;
            }
            self.patches = slots
                .into_iter()
                .map(|(offset, function)| {
                    let slot = (module as usize) + offset;
                    Patch {
                        slot,
                        before: AtomicUsize::from_ptr(slot as *mut usize).load(Ordering::Acquire),
                        after: if function == Function::Color {
                            color
                        } else {
                            background
                        },
                        protection: None,
                    }
                })
                .collect();
            self.install_patches(address)
        }
    }
    fn install_patches(&mut self, address: usize) -> bool {
        for patch in &mut self.patches {
            if !unsafe { patch.replace(true) } {
                self.restore();
                return false;
            }
        }
        self.references = 1;
        self.address = address;
        true
    }
    pub(super) fn release(&mut self) {
        if self.references == 0 {
            return;
        }
        self.references -= 1;
        if self.references == 0 {
            self.restore();
        }
    }
    fn restore(&mut self) -> bool {
        let mut restored = true;
        for patch in self.patches.iter_mut().rev() {
            if !unsafe { patch.replace(false) } {
                restored = false;
            }
        }
        if restored {
            self.patches.clear();
            self.address = 0;
            if self.module != 0 {
                unsafe {
                    FreeLibrary(self.module as HMODULE);
                }
                self.module = 0;
            }
        } else {
            // Keep module and journal alive for another acquire/teardown attempt.
            unsafe {
                windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(w!(
                    "Winlex: task-dialog import restoration failed; retaining restoration journal.\n"
                ));
            }
        }
        restored
    }
}

fn ascii_lower(v: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&v) {
        v + 32
    } else {
        v
    }
}

// The loader has validated these headers; subsequent import reads are bounded.
unsafe fn image<'a>(module: HMODULE) -> Option<&'a [u8]> {
    unsafe {
        let base = module.cast::<u8>();
        if base.cast::<u16>().read_unaligned() != 0x5a4d {
            return None;
        }
        let pe = base.add(0x3c).cast::<u32>().read_unaligned() as usize;
        if !(64..=0x100000).contains(&pe) {
            return None;
        }
        if base.add(pe).cast::<u32>().read_unaligned() != 0x4550 {
            return None;
        }
        let size = base.add(pe + 24 + 56).cast::<u32>().read_unaligned() as usize;
        if size < pe + 264 || size > 0x40000000 {
            return None;
        }
        Some(std::slice::from_raw_parts(base, size))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Function {
    Color,
    Background,
}
fn u16_at(image: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        image.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(image: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        image.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn thunk(image: &[u8], at: usize, width: usize) -> Option<u64> {
    match width {
        4 => Some(u32_at(image, at)? as u64),
        8 => Some(u64::from_le_bytes(
            image.get(at..at.checked_add(8)?)?.try_into().ok()?,
        )),
        _ => None,
    }
}
fn cstr(image: &[u8], at: usize) -> Option<&[u8]> {
    let tail = image.get(at..)?;
    Some(&tail[..tail.iter().take(4096).position(|b| *b == 0)?])
}
fn walk_thunks(
    image: &[u8],
    names: usize,
    slots: usize,
    width: usize,
    result: &mut Vec<(usize, Function)>,
) -> Option<()> {
    if names == 0 || slots == 0 || !slots.is_multiple_of(width) {
        return None;
    }
    for i in 0..65536usize {
        let entry = thunk(image, names.checked_add(i.checked_mul(width)?)?, width)?;
        let slot = slots.checked_add(i.checked_mul(width)?)?;
        thunk(image, slot, width)?;
        if entry == 0 {
            return Some(());
        }
        let ordinal = 1u64 << (width * 8 - 1);
        let function = if entry & ordinal != 0 {
            (entry & 0xffff == 47).then_some(Function::Background)
        } else {
            match cstr(image, usize::try_from(entry).ok()?.checked_add(2)?)? {
                b"GetThemeColor" => Some(Function::Color),
                b"DrawThemeBackgroundEx" => Some(Function::Background),
                _ => None,
            }
        };
        if let Some(function) = function
            && !result.iter().any(|s| s.0 == slot)
        {
            result.push((slot, function));
        }
    }
    None
}

fn find_slots(image: &[u8], width: usize) -> Option<Vec<(usize, Function)>> {
    if u16_at(image, 0)? != 0x5a4d {
        return None;
    }
    let pe = u32_at(image, 0x3c)? as usize;
    if u32_at(image, pe)? != 0x4550 {
        return None;
    }
    let optional = pe.checked_add(24)?;
    let (directories, count) = match (u16_at(image, optional)?, width) {
        (0x10b, 4) => (optional + 96, u32_at(image, optional + 92)? as usize),
        (0x20b, 8) => (optional + 112, u32_at(image, optional + 108)? as usize),
        _ => return None,
    };
    let optional_end = optional.checked_add(u16_at(image, pe + 20)? as usize)?;
    image.get(optional..optional_end)?;
    let mut result = Vec::new();
    for (index, stride, delay) in [(1, 20, false), (13, 32, true)] {
        if index >= count {
            continue;
        }
        if directories.checked_add(index * 8 + 8)? > optional_end {
            return None;
        }
        let rva = u32_at(image, directories + index * 8)? as usize;
        let size = u32_at(image, directories + index * 8 + 4)? as usize;
        if rva == 0 {
            continue;
        }
        let end = rva.checked_add(size)?;
        image.get(rva..end)?;
        let mut at = rva;
        let mut terminated = false;
        while at.checked_add(stride)? <= end {
            let (name, names, slots) = if delay {
                let attributes = u32_at(image, at)?;
                let name = u32_at(image, at + 4)? as usize;
                if name == 0 {
                    terminated = true;
                    break;
                }
                if attributes != 1 {
                    return None;
                }
                (
                    name,
                    u32_at(image, at + 16)? as usize,
                    u32_at(image, at + 12)? as usize,
                )
            } else {
                let name = u32_at(image, at + 12)? as usize;
                if name == 0 {
                    terminated = true;
                    break;
                }
                (
                    name,
                    u32_at(image, at)? as usize,
                    u32_at(image, at + 16)? as usize,
                )
            };
            if cstr(image, name)?.eq_ignore_ascii_case(b"uxtheme.dll") {
                walk_thunks(image, names, slots, width, &mut result)?;
            }
            at += stride;
        }
        if !terminated {
            return None;
        }
    }
    Some(result)
}
