#![allow(non_snake_case)]
use super::*;
use std::{cell::Cell, rc::Weak};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, S_FALSE, S_OK},
        System::{Com::*, Ole::*},
        UI::Accessibility::*,
    },
    core::*,
};
mod abi;
use abi::Msaa;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Node {
    Root,
    Tab(u64),
}

#[implement(Msaa, IEnumVARIANT)]
struct Accessible {
    state: Weak<RefCell<State>>,
    node: Node,
    cursor: Cell<usize>,
}
fn object(s: &Rc<RefCell<State>>, node: Node) -> IAccessible {
    if let Some(Some(value)) = s.borrow_mut().object_slot(node) {
        return value.clone();
    }
    let value: Msaa = Accessible {
        state: Rc::downgrade(s),
        node,
        cursor: Cell::new(0),
    }
    .into();
    let value: IAccessible = value.cast().expect("MSAA interface identity");
    if let Some(slot) = s.borrow_mut().object_slot(node) {
        *slot = Some(value.clone());
    }
    value
}
pub(super) fn result(s: &Rc<RefCell<State>>, w: usize) -> isize {
    let value = object(s, Node::Root);
    unsafe {
        LresultFromObject(
            &IAccessible::IID,
            windows::Win32::Foundation::WPARAM(w),
            &value,
        )
        .0
    }
}
impl Accessible {
    fn live(&self) -> Result<Rc<RefCell<State>>> {
        let s = self.state.upgrade().ok_or_else(|| Error::from(E_FAIL))?;
        let valid = {
            let s = s.borrow();
            !s.hwnd.is_null()
                && match self.node {
                    Node::Root => true,
                    Node::Tab(id) => s.items.iter().any(|i| i.id == id),
                }
        };
        if valid { Ok(s) } else { Err(E_FAIL.into()) }
    }
    fn resolve(&self, child: &VARIANT) -> Result<Node> {
        let id = unsafe {
            let value = &child.as_raw().Anonymous.Anonymous;
            if value.vt != windows::Win32::System::Variant::VT_I4.0 {
                return Err(E_INVALIDARG.into());
            }
            value.Anonymous.lVal
        };
        let s = self.live()?;
        if id == 0 {
            return Ok(self.node);
        }
        match self.node {
            Node::Root if id > 0 => s
                .borrow()
                .items
                .iter()
                .find(|i| i.child == id)
                .map(|i| Node::Tab(i.id))
                .ok_or_else(|| E_INVALIDARG.into()),
            _ => Err(E_INVALIDARG.into()),
        }
    }
    fn children(&self) -> Result<Vec<Node>> {
        let s = self.live()?;
        let s = s.borrow();
        Ok(match self.node {
            Node::Root => s.items.iter().map(|i| Node::Tab(i.id)).collect(),
            _ => vec![],
        })
    }
    fn variant(&self, node: Node) -> Result<VARIANT> {
        Ok(object(&self.live()?, node).cast::<IDispatch>()?.into())
    }
    fn native(&self) -> Result<IAccessible> {
        let hwnd = self.live()?.borrow().hwnd;
        let mut raw = ptr::null_mut();
        unsafe {
            CreateStdAccessibleObject(
                windows::Win32::Foundation::HWND(hwnd),
                OBJID_CLIENT,
                &IAccessible::IID,
                &mut raw,
            )?;
            Ok(IAccessible::from_raw(raw))
        }
    }
}
impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(1)
    }
    fn GetTypeInfo(&self, index: u32, lcid: u32) -> Result<ITypeInfo> {
        unsafe { self.native()?.GetTypeInfo(index, lcid) }
    }
    fn GetIDsOfNames(
        &self,
        iid: *const GUID,
        names: *const PCWSTR,
        count: u32,
        lcid: u32,
        ids: *mut i32,
    ) -> Result<()> {
        unsafe { self.native()?.GetIDsOfNames(iid, names, count, lcid, ids) }
    }
    fn Invoke(
        &self,
        member: i32,
        _iid: *const GUID,
        lcid: u32,
        flags: DISPATCH_FLAGS,
        params: *const DISPPARAMS,
        result: *mut VARIANT,
        exception: *mut EXCEPINFO,
        arg: *mut u32,
    ) -> Result<()> {
        let s = self.live()?;
        let this = object(&s, self.node);
        unsafe {
            DispInvoke(
                this.as_raw(),
                &self.GetTypeInfo(0, lcid)?,
                member,
                flags.0,
                params.cast_mut(),
                result,
                exception,
                arg,
            )
        }
    }
}
impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> Result<IDispatch> {
        let s = self.live()?;
        match self.node {
            Node::Root => unsafe { self.native()?.accParent() },
            Node::Tab(_) => object(&s, Node::Root).cast(),
        }
    }
    fn accChildCount(&self) -> Result<i32> {
        Ok(self.children()?.len() as i32)
    }
    fn get_accChild(&self, child: &VARIANT) -> Result<IDispatch> {
        object(&self.live()?, self.resolve(child)?).cast()
    }
    fn get_accName(&self, child: &VARIANT) -> Result<BSTR> {
        let node = self.resolve(child)?;
        let s = self.live()?;
        let s = s.borrow();
        Ok(match node {
            Node::Root => s.name.as_str(),
            Node::Tab(id) => &s.items.iter().find(|i| i.id == id).unwrap().title,
        }
        .into())
    }
    fn get_accValue(&self, child: &VARIANT) -> Result<BSTR> {
        self.resolve(child)?;
        Err(S_FALSE.into())
    }
    fn get_accDescription(&self, child: &VARIANT) -> Result<BSTR> {
        let node = self.resolve(child)?;
        let s = self.live()?;
        let s = s.borrow();
        Ok(match node {
            Node::Root => "",
            Node::Tab(id) => &s.items.iter().find(|i| i.id == id).unwrap().description,
        }
        .into())
    }
    fn get_accRole(&self, child: &VARIANT) -> Result<VARIANT> {
        Ok((match self.resolve(child)? {
            Node::Root => ROLE_SYSTEM_PAGETABLIST,
            Node::Tab(_) => ROLE_SYSTEM_PAGETAB,
        } as i32)
            .into())
    }
    fn get_accState(&self, child: &VARIANT) -> Result<VARIANT> {
        let node = self.resolve(child)?;
        let s = self.live()?;
        let s = s.borrow();
        let mut flags = 0;
        if unsafe { IsWindowEnabled(s.hwnd) } == 0 {
            flags |= STATE_SYSTEM_UNAVAILABLE;
        }
        if unsafe { IsWindowVisible(s.hwnd) } == 0 {
            flags |= STATE_SYSTEM_INVISIBLE;
        }
        match node {
            Node::Root => {
                flags |= STATE_SYSTEM_FOCUSABLE;
                if unsafe { GetFocus() } == s.hwnd && s.focused.is_none() {
                    flags |= STATE_SYSTEM_FOCUSED;
                }
            }
            Node::Tab(id) => {
                flags |= STATE_SYSTEM_FOCUSABLE | STATE_SYSTEM_SELECTABLE;
                if s.selected == Some(id) {
                    flags |= STATE_SYSTEM_SELECTED;
                }
                if s.focused == Some(id) && unsafe { GetFocus() } == s.hwnd {
                    flags |= STATE_SYSTEM_FOCUSED;
                }
                if let Some(r) = s.rect(id)
                    && (r.left < 0 || r.right > client(s.hwnd).right)
                {
                    flags |= STATE_SYSTEM_OFFSCREEN;
                }
            }
        }
        Ok((flags as i32).into())
    }
    fn get_accHelp(&self, child: &VARIANT) -> Result<BSTR> {
        self.resolve(child)?;
        Err(S_FALSE.into())
    }
    fn get_accHelpTopic(&self, _file: *mut BSTR, child: &VARIANT) -> Result<i32> {
        self.resolve(child)?;
        Err(S_FALSE.into())
    }
    fn get_accKeyboardShortcut(&self, child: &VARIANT) -> Result<BSTR> {
        self.resolve(child)?;
        Err(S_FALSE.into())
    }
    fn accFocus(&self) -> Result<VARIANT> {
        let s = self.live()?;
        let (focused, id) = {
            let s = s.borrow();
            (unsafe { GetFocus() } == s.hwnd, s.focused)
        };
        if !focused {
            return Ok(VARIANT::new());
        }
        match (self.node, id) {
            (Node::Root, Some(id)) => self.variant(Node::Tab(id)),
            (Node::Root, None) => Ok(0i32.into()),
            (Node::Tab(id), Some(selected)) if id == selected => Ok(0i32.into()),
            _ => Ok(VARIANT::new()),
        }
    }
    fn accSelection(&self) -> Result<VARIANT> {
        let id = self.live()?.borrow().selected;
        match (self.node, id) {
            (Node::Root, Some(id)) => self.variant(Node::Tab(id)),
            (Node::Tab(id), Some(selected)) if id == selected => Ok(0i32.into()),
            _ => Ok(VARIANT::new()),
        }
    }
    fn get_accDefaultAction(&self, child: &VARIANT) -> Result<BSTR> {
        match self.resolve(child)? {
            Node::Root => Err(S_FALSE.into()),
            Node::Tab(_) => Ok(self.live()?.borrow().select_label.as_str().into()),
        }
    }
    fn accSelect(&self, flags: i32, child: &VARIANT) -> Result<()> {
        if flags & !((SELFLAG_TAKEFOCUS | SELFLAG_TAKESELECTION) as i32) != 0 {
            return Err(E_INVALIDARG.into());
        }
        let node = self.resolve(child)?;
        let shared = self.live()?;
        let hwnd = shared.borrow().hwnd;
        if flags & SELFLAG_TAKESELECTION as i32 != 0 {
            let Node::Tab(id) = node else {
                return Err(E_INVALIDARG.into());
            };
            request(hwnd, SELECT, id, 0, POINT { x: 0, y: 0 });
        }
        if flags & SELFLAG_TAKEFOCUS as i32 != 0 {
            let id = match node {
                Node::Root => None,
                Node::Tab(id) => Some(id),
            };
            focus_item(&shared, id);
        }
        Ok(())
    }
    fn accLocation(
        &self,
        left: *mut i32,
        top: *mut i32,
        width: *mut i32,
        height: *mut i32,
        child: &VARIANT,
    ) -> Result<()> {
        if left.is_null() || top.is_null() || width.is_null() || height.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let node = self.resolve(child)?;
        let s = self.live()?;
        let s = s.borrow();
        let r = match node {
            Node::Root => client(s.hwnd),
            Node::Tab(id) => s.rect(id).unwrap(),
        };
        let mut p = POINT {
            x: r.left,
            y: r.top,
        };
        unsafe {
            ClientToScreen(s.hwnd, &mut p);
            left.write(p.x);
            top.write(p.y);
            width.write(r.right - r.left);
            height.write(r.bottom - r.top);
        }
        Ok(())
    }
    fn accNavigate(&self, dir: i32, child: &VARIANT) -> Result<VARIANT> {
        let node = self.resolve(child)?;
        let s = self.live()?;
        let target = {
            let s = s.borrow();
            match (dir as u32, node) {
                (NAVDIR_FIRSTCHILD | NAVDIR_LASTCHILD, Node::Root) => {
                    if dir as u32 == NAVDIR_FIRSTCHILD {
                        s.items.first()
                    } else {
                        s.items.last()
                    }
                    .map(|i| Node::Tab(i.id))
                }
                (NAVDIR_NEXT | NAVDIR_RIGHT | NAVDIR_PREVIOUS | NAVDIR_LEFT, Node::Tab(id)) => {
                    let index = s.items.iter().position(|i| i.id == id).unwrap();
                    let next = if matches!(dir as u32, NAVDIR_NEXT | NAVDIR_RIGHT) {
                        index.checked_add(1)
                    } else {
                        index.checked_sub(1)
                    };
                    next.and_then(|i| s.items.get(i)).map(|i| Node::Tab(i.id))
                }
                _ => None,
            }
        };
        target.map_or(Err(S_FALSE.into()), |node| self.variant(node))
    }
    fn accHitTest(&self, x: i32, y: i32) -> Result<VARIANT> {
        let s = self.live()?;
        let (hit, inside) = {
            let s = s.borrow();
            let mut p = POINT { x, y };
            unsafe {
                ScreenToClient(s.hwnd, &mut p);
            }
            (s.hit(p), contains(client(s.hwnd), p))
        };
        if !inside {
            return Ok(VARIANT::new());
        }
        match (self.node, hit) {
            (Node::Root, Some(id)) => self.variant(Node::Tab(id)),
            (Node::Root, None) => Ok(0i32.into()),
            (Node::Tab(id), Some(hit)) if id == hit => Ok(0i32.into()),
            _ => Ok(VARIANT::new()),
        }
    }
    fn accDoDefaultAction(&self, child: &VARIANT) -> Result<()> {
        let node = self.resolve(child)?;
        let hwnd = self.live()?.borrow().hwnd;
        match node {
            Node::Tab(id) => request(hwnd, SELECT, id, 0, POINT { x: 0, y: 0 }),
            Node::Root => return Err(E_NOTIMPL.into()),
        }
        Ok(())
    }
    fn put_accName(&self, _child: &VARIANT, _name: &BSTR) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn put_accValue(&self, _child: &VARIANT, _value: &BSTR) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}
impl IEnumVARIANT_Impl for Accessible_Impl {
    fn Next(&self, count: u32, values: *mut VARIANT, fetched: *mut u32) -> HRESULT {
        if values.is_null() || (fetched.is_null() && count != 1) {
            return E_INVALIDARG;
        }
        if !fetched.is_null() {
            unsafe {
                fetched.write(0);
            }
        }
        let Ok(children) = self.children() else {
            return E_FAIL;
        };
        let start = self.cursor.get().min(children.len());
        let end = start.saturating_add(count as usize).min(children.len());
        for (index, node) in children[start..end].iter().enumerate() {
            let Ok(value) = self.variant(*node) else {
                return E_FAIL;
            };
            unsafe {
                values.add(index).write(value);
            }
        }
        self.cursor.set(end);
        if !fetched.is_null() {
            unsafe {
                fetched.write((end - start) as u32);
            }
        }
        if end - start == count as usize {
            S_OK
        } else {
            S_FALSE
        }
    }
    fn Skip(&self, count: u32) -> HRESULT {
        let Ok(children) = self.children() else {
            return E_FAIL;
        };
        let end = self.cursor.get().saturating_add(count as usize);
        self.cursor.set(end.min(children.len()));
        if end <= children.len() { S_OK } else { S_FALSE }
    }
    fn Reset(&self) -> Result<()> {
        self.live()?;
        self.cursor.set(0);
        Ok(())
    }
    fn Clone(&self) -> Result<IEnumVARIANT> {
        self.live()?;
        Ok(Accessible {
            state: self.state.clone(),
            node: self.node,
            cursor: Cell::new(self.cursor.get()),
        }
        .into())
    }
}
