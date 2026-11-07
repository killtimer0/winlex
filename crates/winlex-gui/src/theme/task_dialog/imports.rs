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
static COLOR: AtomicUsize = AtomicUsize::new(0);
static BACKGROUND: AtomicUsize = AtomicUsize::new(0);

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
    unsafe { mem::transmute::<usize, ColorFn>(COLOR.load(Ordering::Acquire)) }
}
pub(super) unsafe fn original_background() -> BackgroundFn {
    unsafe { mem::transmute::<usize, BackgroundFn>(BACKGROUND.load(Ordering::Acquire)) }
}

struct Patch {
    slot: usize,
    before: usize,
    after: usize,
    protection: Option<u32>,
}
unsafe fn protect_slot(pointer: *mut usize, protection: u32, old: &mut u32) -> bool {
    #[cfg(test)]
    if tests::fail_protection() {
        return false;
    }
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
            let Some((original_color, original_background)) = exports() else {
                self.restore();
                return false;
            };
            COLOR.store(original_color, Ordering::Release);
            BACKGROUND.store(original_background, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READONLY,
        VirtualAlloc, VirtualFree, VirtualQuery,
    };

    fn put32(image: &mut [u8], at: usize, value: u32) {
        image[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture(width: usize, delay: bool) -> Vec<u8> {
        let mut image = vec![0; 4096];
        image[..2].copy_from_slice(&0x5a4du16.to_le_bytes());
        put32(&mut image, 0x3c, 0x80);
        put32(&mut image, 0x80, 0x4550);
        let optional = 0x80 + 24;
        let (magic, directories) = if width == 4 {
            (0x10bu16, optional + 96)
        } else {
            (0x20bu16, optional + 112)
        };
        image[optional..optional + 2].copy_from_slice(&magic.to_le_bytes());
        image[0x80 + 20..0x80 + 22]
            .copy_from_slice(&(if width == 4 { 224u16 } else { 240u16 }).to_le_bytes());
        put32(&mut image, directories - 4, 16);
        let index = if delay { 13 } else { 1 };
        put32(&mut image, directories + index * 8, 0x200);
        put32(
            &mut image,
            directories + index * 8 + 4,
            if delay { 64 } else { 40 },
        );
        if delay {
            put32(&mut image, 0x200, 1);
            put32(&mut image, 0x204, 0x300);
            put32(&mut image, 0x20c, 0x500);
            put32(&mut image, 0x210, 0x400);
        } else {
            put32(&mut image, 0x200, 0x400);
            put32(&mut image, 0x20c, 0x300);
            put32(&mut image, 0x210, 0x500);
        }
        image[0x300..0x30c].copy_from_slice(b"uxtheme.dll\0");
        put32(&mut image, 0x400, 0x600);
        let ordinal = (1u64 << (width * 8 - 1)) | 47;
        image[0x400 + width..0x400 + width * 2].copy_from_slice(&ordinal.to_le_bytes()[..width]);
        image[0x602..0x610].copy_from_slice(b"GetThemeColor\0");
        image
    }

    #[derive(Default)]
    struct ProtectionFaults {
        calls: usize,
        fail: Vec<usize>,
    }
    thread_local! {
        static PROTECTION_FAULTS: RefCell<ProtectionFaults> = RefCell::default();
    }
    pub(super) fn fail_protection() -> bool {
        PROTECTION_FAULTS.with_borrow_mut(|faults| {
            faults.calls += 1;
            faults.fail.contains(&faults.calls)
        })
    }
    struct ProtectionFaultScope;
    impl ProtectionFaultScope {
        fn new(fail: &[usize]) -> Self {
            PROTECTION_FAULTS.with_borrow_mut(|faults| {
                assert!(faults.fail.is_empty(), "fault scopes must not nest");
                *faults = ProtectionFaults {
                    calls: 0,
                    fail: fail.to_vec(),
                };
            });
            Self
        }
    }
    impl Drop for ProtectionFaultScope {
        fn drop(&mut self) {
            PROTECTION_FAULTS.with_borrow_mut(|faults| *faults = ProtectionFaults::default());
        }
    }

    // Native pages and pointer-width slots, but never process-wide system IATs.
    struct InstallFixture {
        memory: *mut std::ffi::c_void,
        hooks: Hooks,
        slots: Vec<(usize, usize)>,
    }
    impl InstallFixture {
        fn new(delay: bool) -> Self {
            unsafe {
                let width = mem::size_of::<usize>();
                let image = fixture(width, delay);
                let slots = find_slots(&image, width).unwrap();
                let memory =
                    VirtualAlloc(ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                assert!(!memory.is_null());
                let mut fixture = Self {
                    memory,
                    hooks: Hooks::new(),
                    slots: Vec::new(),
                };
                ptr::copy_nonoverlapping(image.as_ptr(), memory.cast(), image.len());
                for (index, (offset, _)) in slots.into_iter().enumerate() {
                    let slot = memory as usize + offset;
                    let before = 0x1234 + index;
                    (slot as *mut usize).write(before);
                    fixture.slots.push((slot, before));
                }
                let mut old = 0;
                assert_ne!(VirtualProtect(memory, 4096, PAGE_READONLY, &mut old), 0);
                fixture.prepare();
                fixture
            }
        }
        fn prepare(&mut self) {
            assert_eq!(self.hooks.module, 0);
            assert!(self.hooks.patches.is_empty());
            // Own a real loader reference so the production rollback releases it.
            let module = unsafe { LoadLibraryW(w!("kernel32.dll")) };
            assert!(!module.is_null());
            self.hooks.module = module as usize;
            self.hooks.patches = self
                .slots
                .iter()
                .map(|&(slot, before)| Patch {
                    slot,
                    before,
                    after: before + 0x1000,
                    protection: None,
                })
                .collect();
        }
        fn assert_slots(&self, installed: bool, protection: u32) {
            for &(slot, before) in &self.slots {
                unsafe {
                    assert_eq!(
                        (slot as *const usize).read(),
                        before + if installed { 0x1000 } else { 0 }
                    );
                    let mut info: MEMORY_BASIC_INFORMATION = mem::zeroed();
                    assert_ne!(
                        VirtualQuery(
                            slot as *const _,
                            &mut info,
                            mem::size_of::<MEMORY_BASIC_INFORMATION>()
                        ),
                        0
                    );
                    assert_eq!(info.Protect, protection);
                }
            }
        }
        fn assert_retry(&mut self) {
            assert_eq!(self.hooks.references, 0);
            assert_eq!(self.hooks.address, 0);
            assert_eq!(
                self.hooks.module, 0,
                "rollback must release its module guard"
            );
            assert!(self.hooks.patches.is_empty());
            self.assert_slots(false, PAGE_READONLY);
            self.prepare();
            assert!(self.hooks.install_patches(self.memory as usize));
            self.assert_slots(true, PAGE_READONLY);
            self.hooks.release();
            self.assert_slots(false, PAGE_READONLY);
            assert_eq!(self.hooks.module, 0);
            assert!(self.hooks.patches.is_empty());
        }
    }
    impl Drop for InstallFixture {
        fn drop(&mut self) {
            assert!(self.hooks.restore());
            unsafe {
                VirtualFree(self.memory, 0, MEM_RELEASE);
            }
        }
    }

    #[test]
    fn install_failure_after_first_slot_restores_ordinary_and_delay_imports() {
        for delay in [false, true] {
            let mut fixture = InstallFixture::new(delay);
            {
                // First slot installs and regains read-only protection; opening
                // the second slot fails, entering the real transaction rollback.
                let _fault = ProtectionFaultScope::new(&[3]);
                assert!(!fixture.hooks.install_patches(fixture.memory as usize));
            }
            fixture.assert_retry();
        }
    }

    #[test]
    fn install_failure_restoring_protection_rolls_back_pointer_and_protection() {
        for delay in [false, true] {
            let mut fixture = InstallFixture::new(delay);
            {
                let _fault = ProtectionFaultScope::new(&[2]);
                assert!(!fixture.hooks.install_patches(fixture.memory as usize));
            }
            fixture.assert_retry();
        }
    }

    #[test]
    fn install_failure_retains_journal_and_module_until_protection_retry() {
        for delay in [false, true] {
            let mut fixture = InstallFixture::new(delay);
            let module = fixture.hooks.module;
            {
                // Fail the first slot's final protection change, then the same
                // change during rollback (which visits the untouched slot first).
                let _fault = ProtectionFaultScope::new(&[2, 6]);
                assert!(!fixture.hooks.install_patches(fixture.memory as usize));
            }
            fixture.assert_slots(false, PAGE_READWRITE);
            assert_eq!(
                fixture.hooks.references, 0,
                "failed installation is not active"
            );
            assert_eq!(
                fixture.hooks.module, module,
                "retry must keep the loader guard"
            );
            assert!(
                fixture
                    .hooks
                    .patches
                    .iter()
                    .any(|patch| patch.protection == Some(PAGE_READONLY))
            );
            assert!(fixture.hooks.restore());
            fixture.assert_retry();
        }
    }

    #[test]
    fn discovers_only_requested_uxtheme_import_slots_for_both_pe_formats() {
        for width in [4, 8] {
            for delay in [false, true] {
                let mut image = fixture(width, delay);
                assert_eq!(
                    find_slots(&image, width),
                    Some(vec![
                        (0x500, Function::Color),
                        (0x500 + width, Function::Background)
                    ])
                );
                image[0x300..0x30c].copy_from_slice(b"another.dll\0");
                assert_eq!(find_slots(&image, width), Some(Vec::new()));
            }
        }
    }

    #[test]
    fn rejects_truncated_or_out_of_image_imports_without_partial_results() {
        for width in [4, 8] {
            let image = fixture(width, true);
            for length in [0, 64, 0x100, 0x200, 0x410, 0x60f] {
                assert!(find_slots(&image[..length], width).is_none());
            }
            let mut image = image;
            put32(&mut image, 0x210, u32::MAX);
            assert!(find_slots(&image, width).is_none());
        }
    }

    #[test]
    fn last_overlapping_scope_restores_value_and_page_protection() {
        unsafe {
            let memory = VirtualAlloc(ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            assert!(!memory.is_null());
            struct Allocation(*mut std::ffi::c_void);
            impl Drop for Allocation {
                fn drop(&mut self) {
                    unsafe {
                        VirtualFree(self.0, 0, MEM_RELEASE);
                    }
                }
            }
            let _allocation = Allocation(memory);
            (memory as *mut usize).write(0x1234);
            let mut old = 0;
            assert_ne!(VirtualProtect(memory, 4096, PAGE_READONLY, &mut old), 0);
            let mut patch = Patch {
                slot: memory as usize,
                before: 0x1234,
                after: 0x5678,
                protection: None,
            };
            assert!(patch.replace(true));
            assert_eq!((memory as *const usize).read(), 0x5678);
            let mut hooks = Hooks {
                references: 1,
                module: 0,
                address: 0x42,
                patches: vec![patch],
            };
            assert!(hooks.acquire(0x42, 0, 0));
            assert!(!hooks.acquire(0x43, 0, 0));
            hooks.release();
            assert_eq!(
                (memory as *const usize).read(),
                0x5678,
                "an inner scope must not detach an outer scope's imports"
            );
            let mut info: MEMORY_BASIC_INFORMATION = mem::zeroed();
            assert_ne!(
                VirtualQuery(
                    memory,
                    &mut info,
                    mem::size_of::<MEMORY_BASIC_INFORMATION>()
                ),
                0
            );
            assert_eq!(info.Protect, PAGE_READONLY);
            hooks.release();
            assert!(hooks.patches.is_empty());
            assert_eq!((memory as *const usize).read(), 0x1234);
            assert_ne!(
                VirtualQuery(
                    memory,
                    &mut info,
                    mem::size_of::<MEMORY_BASIC_INFORMATION>()
                ),
                0
            );
            assert_eq!(info.Protect, PAGE_READONLY);
        }
    }

    #[repr(C)]
    struct ActCtx {
        size: u32,
        flags: u32,
        source: *const u16,
        processor: u16,
        language: u16,
        directory: *const u16,
        resource: *const u16,
        application: *const u16,
        module: HMODULE,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateActCtxW(context: *const ActCtx) -> HANDLE;
        fn ActivateActCtx(context: HANDLE, cookie: *mut usize) -> BOOL;
        fn DeactivateActCtx(flags: u32, cookie: usize) -> BOOL;
        fn ReleaseActCtx(context: HANDLE);
    }
    struct Activation {
        handle: HANDLE,
        cookie: usize,
        module: HMODULE,
    }
    impl Drop for Activation {
        fn drop(&mut self) {
            unsafe {
                if !self.module.is_null() {
                    FreeLibrary(self.module);
                }
                if self.cookie != 0 {
                    DeactivateActCtx(0, self.cookie);
                }
                ReleaseActCtx(self.handle);
            }
        }
    }
    impl Activation {
        fn new() -> Self {
            use std::os::windows::ffi::OsStrExt;
            let manifest = std::env::var_os("WINLEX_TEST_GUI_MANIFEST")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/gui.manifest")
                });
            let source: Vec<_> = manifest.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                let context = ActCtx {
                    size: mem::size_of::<ActCtx>() as u32,
                    source: source.as_ptr(),
                    ..mem::zeroed()
                };
                let handle = CreateActCtxW(&context);
                assert_ne!(handle, INVALID_HANDLE_VALUE);
                let mut activation = Self {
                    handle,
                    cookie: 0,
                    module: ptr::null_mut(),
                };
                assert_ne!(ActivateActCtx(handle, &mut activation.cookie), 0);
                activation.module = LoadLibraryW(w!("comctl32.dll"));
                assert!(!activation.module.is_null());
                activation
            }
        }
    }
    unsafe fn snapshot(module: HMODULE) -> Vec<(usize, usize, u32)> {
        unsafe {
            find_slots(image(module).unwrap(), mem::size_of::<usize>())
                .unwrap()
                .into_iter()
                .map(|(offset, _)| {
                    let slot = module as usize + offset;
                    let mut info: MEMORY_BASIC_INFORMATION = mem::zeroed();
                    assert_ne!(
                        VirtualQuery(
                            slot as *const _,
                            &mut info,
                            mem::size_of::<MEMORY_BASIC_INFORMATION>()
                        ),
                        0
                    );
                    (
                        slot,
                        AtomicUsize::from_ptr(slot as *mut usize).load(Ordering::Acquire),
                        info.Protect,
                    )
                })
                .collect()
        }
    }

    // No dialog runs while faults are armed. Run in a fresh process, alone:
    // the slots belong to this process's activation-context-selected comctl32.
    #[test]
    #[ignore = "process-wide IAT failure injection; run this test alone in a fresh process"]
    fn native_install_failure_acquire_recovers_retained_journal_and_real_imports() {
        if !super::super::super::supported() {
            return;
        }
        struct RestoreHooks(Hooks);
        impl Drop for RestoreHooks {
            fn drop(&mut self) {
                // Fault scopes are nested inside this owner, so their Drop has
                // disabled injection before cleanup, including during unwinding.
                assert!(
                    self.0.restore(),
                    "native IAT cleanup must restore all imports"
                );
            }
        }
        let activation = Activation::new();
        assert_eq!(
            super::super::HOOKS.lock().unwrap().references,
            0,
            "run alone"
        );
        let mut owner = RestoreHooks(Hooks::new());
        let hooks = &mut owner.0;
        unsafe {
            let address = GetProcAddress(activation.module, c"TaskDialogIndirect".as_ptr().cast())
                .expect("v6 common controls must export TaskDialogIndirect")
                as usize;
            let color = super::super::theme_color as *const () as usize;
            let background = super::super::theme_background as *const () as usize;
            let original = snapshot(activation.module);
            assert!(original.len() >= 2);
            assert!(
                original
                    .iter()
                    .all(|slot| slot.1 != color && slot.1 != background)
            );
            {
                // Fail opening the second slot after the first was installed.
                let _fault = ProtectionFaultScope::new(&[3]);
                assert!(!hooks.acquire(address, color, background));
            }
            assert_eq!(hooks.references, 0, "failed acquisition must not be active");
            assert_eq!(snapshot(activation.module), original);
            assert!(hooks.patches.is_empty());
            assert_eq!(
                hooks.module, 0,
                "completed rollback releases its loader guard"
            );

            {
                // Fail the first slot's protection restoration during install
                // and again during rollback. Rollback visits every discovered
                // slot in reverse, so derive the second fault from this image,
                // rather than assuming that comctl32 has exactly two imports.
                let _fault = ProtectionFaultScope::new(&[2, 2 + 2 * original.len()]);
                assert!(!hooks.acquire(address, color, background));
            }
            assert_eq!(hooks.references, 0, "failed acquisition must not be active");
            assert_eq!(hooks.module, activation.module as usize);
            assert!(hooks.patches.iter().any(|patch| patch.protection.is_some()));
            assert!(
                snapshot(activation.module)
                    .iter()
                    .zip(&original)
                    .all(|(current, before)| { current.0 == before.0 && current.1 == before.1 }),
                "failed rollback must leave no callback installed"
            );

            // Deliberately do not restore or rebuild the journal here: acquire
            // must recover the retained transaction before installing anew.
            assert!(hooks.acquire(address, color, background));
            assert_eq!(hooks.references, 1);
            assert_eq!(hooks.address, address);
            let expected: Vec<_> =
                find_slots(image(activation.module).unwrap(), mem::size_of::<usize>())
                    .unwrap()
                    .into_iter()
                    .zip(&original)
                    .map(|((offset, function), before)| {
                        assert_eq!(activation.module as usize + offset, before.0);
                        (
                            before.0,
                            if function == Function::Color {
                                color
                            } else {
                                background
                            },
                            before.2,
                        )
                    })
                    .collect();
            assert_eq!(snapshot(activation.module), expected);
            hooks.release();
            assert_eq!(snapshot(activation.module), original);
            assert_eq!(hooks.references, 0);
            assert_eq!(hooks.address, 0);
            assert_eq!(hooks.module, 0);
            assert!(hooks.patches.is_empty());
        }
    }

    // This test must not race other native-control tests: the assertion concerns
    // process-wide imports, not just this thread's callback policy.
    #[test]
    #[ignore = "process-wide IAT restoration test; run this test alone"]
    fn native_task_dialog_nested_guards_restore_real_slots_and_protections() {
        if !super::super::super::supported() {
            return;
        }
        unsafe extern "system" fn close_dialog(
            hwnd: HWND,
            notification: TASKDIALOG_NOTIFICATIONS,
            elapsed: WPARAM,
            _: LPARAM,
            _: isize,
        ) -> HRESULT {
            unsafe {
                if notification == TDN_DIALOG_CONSTRUCTED {
                    super::super::apply_native(hwnd);
                    SetWindowPos(
                        hwnd,
                        ptr::null_mut(),
                        -32000,
                        -32000,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                } else if notification == TDN_CREATED
                    || notification == TDN_TIMER && elapsed >= 1000
                {
                    super::super::apply_native(hwnd);
                    ShowWindow(hwnd, SW_HIDE);
                    SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, IDOK as usize, 0);
                }
            }
            S_OK
        }
        let activation = Activation::new();
        unsafe {
            let address = GetProcAddress(activation.module, c"TaskDialogIndirect".as_ptr().cast())
                .expect("v6 common controls must export TaskDialogIndirect");
            type TaskDialog = unsafe extern "system" fn(
                *const TASKDIALOGCONFIG,
                *mut i32,
                *mut i32,
                *mut BOOL,
            ) -> HRESULT;
            let task: TaskDialog = mem::transmute(address);
            let original = snapshot(activation.module);
            assert!(original.len() >= 2);
            assert_eq!(
                super::super::HOOKS.lock().unwrap().references,
                0,
                "run this process-wide test alone"
            );
            // Exercise both guarded calls and restored delay-loader stubs. Each
            // unguarded call may resolve a cold slot; the next acquire must save
            // and restore that new value instead of reusing a stale snapshot.
            for _ in 0..2 {
                let original = snapshot(activation.module);
                let outer = super::super::HookGuard::new(address as usize);
                assert!(outer.is_active());
                let installed = snapshot(activation.module);
                assert!(
                    original
                        .iter()
                        .zip(&installed)
                        .all(|(before, after)| before.0 == after.0
                            && before.1 != after.1
                            && before.2 == after.2)
                );
                let inner = super::super::HookGuard::new(address as usize);
                assert!(inner.is_active());
                let config = TASKDIALOGCONFIG {
                    cbSize: mem::size_of::<TASKDIALOGCONFIG>() as u32,
                    dwFlags: TDF_CALLBACK_TIMER,
                    dwCommonButtons: TDCBF_OK_BUTTON,
                    pszWindowTitle: w!("Winlex native hook test"),
                    pszContent: w!("Native task dialog lifetime"),
                    pfCallback: Some(close_dialog),
                    ..mem::zeroed()
                };
                let mut button = 0;
                assert!(task(&config, &mut button, ptr::null_mut(), ptr::null_mut()) >= 0);
                assert_eq!(button, IDOK);
                drop(inner);
                assert_eq!(
                    snapshot(activation.module),
                    installed,
                    "inner guard must not detach outer guard imports"
                );
                drop(outer);
                assert_eq!(
                    snapshot(activation.module),
                    original,
                    "last guard must restore every original pointer and page protection"
                );
                button = 0;
                assert!(task(&config, &mut button, ptr::null_mut(), ptr::null_mut()) >= 0);
                assert_eq!(
                    button, IDOK,
                    "restored native delay-loader stubs must remain callable"
                );
            }
        }
    }
}
