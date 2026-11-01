use super::*;
use std::{ffi::c_void, mem::MaybeUninit};

windows_core::imp::define_interface!(Msaa, Msaa_Vtbl, 0x618736e0_3c3d_11cf_810c_00aa00389b71);
windows_core::imp::interface_hierarchy!(Msaa, IUnknown, IDispatch);
impl RuntimeName for Msaa {}

// windows 0.58 only writes return values on Ok; S_FALSE still requires empty outputs.
#[repr(transparent)]
#[allow(non_camel_case_types)]
pub struct Msaa_Vtbl(IAccessible_Vtbl);
impl Msaa_Vtbl {
    pub fn matches(iid: &GUID) -> bool {
        IAccessible_Vtbl::matches(iid)
    }
    pub const fn new<Identity: IAccessible_Impl + IUnknownImpl, const OFFSET: isize>() -> Self {
        macro_rules! empty_string {
            ($name:ident) => {
                unsafe extern "system" fn $name<
                    I: IAccessible_Impl + IUnknownImpl,
                    const O: isize,
                >(
                    this: *mut c_void,
                    child: MaybeUninit<VARIANT>,
                    out: *mut MaybeUninit<BSTR>,
                ) -> HRESULT {
                    if out.is_null() {
                        return windows::Win32::Foundation::E_POINTER;
                    }
                    unsafe {
                        out.write(MaybeUninit::new(BSTR::new()));
                        (IAccessible_Vtbl::new::<I, O>().$name)(this, child, out)
                    }
                }
            };
        }
        empty_string!(get_accValue);
        empty_string!(get_accHelp);
        empty_string!(get_accKeyboardShortcut);
        empty_string!(get_accDefaultAction);
        unsafe extern "system" fn help<I: IAccessible_Impl + IUnknownImpl, const O: isize>(
            this: *mut c_void,
            file: *mut MaybeUninit<BSTR>,
            child: MaybeUninit<VARIANT>,
            topic: *mut i32,
        ) -> HRESULT {
            if file.is_null() || topic.is_null() {
                return windows::Win32::Foundation::E_POINTER;
            }
            unsafe {
                file.write(MaybeUninit::new(BSTR::new()));
                topic.write(0);
                (IAccessible_Vtbl::new::<I, O>().get_accHelpTopic)(this, file, child, topic)
            }
        }
        unsafe extern "system" fn navigate<I: IAccessible_Impl + IUnknownImpl, const O: isize>(
            this: *mut c_void,
            direction: i32,
            start: MaybeUninit<VARIANT>,
            out: *mut MaybeUninit<VARIANT>,
        ) -> HRESULT {
            if out.is_null() {
                return windows::Win32::Foundation::E_POINTER;
            }
            unsafe {
                out.write(MaybeUninit::new(VARIANT::new()));
                (IAccessible_Vtbl::new::<I, O>().accNavigate)(this, direction, start, out)
            }
        }
        let mut table = IAccessible_Vtbl::new::<Identity, OFFSET>();
        table.get_accValue = get_accValue::<Identity, OFFSET>;
        table.get_accHelp = get_accHelp::<Identity, OFFSET>;
        table.get_accKeyboardShortcut = get_accKeyboardShortcut::<Identity, OFFSET>;
        table.get_accDefaultAction = get_accDefaultAction::<Identity, OFFSET>;
        table.get_accHelpTopic = help::<Identity, OFFSET>;
        table.accNavigate = navigate::<Identity, OFFSET>;
        Self(table)
    }
}
