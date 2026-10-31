"""MSAA client using the system proxy, independent of the target's bitness."""
import ctypes as c
from ctypes import wintypes as w
from contextlib import contextmanager
import uuid
import pythoncom
from . import native as n


class Value(c.Union):
    _fields_ = [("number", c.c_long), ("pointer", c.c_void_p),
                ("record", c.c_void_p * 2), ("alignment", c.c_double)]


class Variant(c.Structure):
    _fields_ = [("type", c.c_ushort), ("reserved", c.c_ushort * 3), ("value", Value)]


oleacc = c.WinDLL("oleacc")
oleaut = c.WinDLL("oleaut32")
accessible_window = n.api(oleacc, "AccessibleObjectFromWindow", c.c_long,
                          w.HWND, w.DWORD, c.c_void_p, c.POINTER(c.c_void_p))
accessible_children = n.api(oleacc, "AccessibleChildren", c.c_long, c.c_void_p,
                            c.c_long, c.c_long, c.POINTER(Variant), c.POINTER(c.c_long))
clear_variant = n.api(oleaut, "VariantClear", c.c_long, c.POINTER(Variant))
free_string = n.api(oleaut, "SysFreeString", None, c.c_void_p)
IID = (c.c_ubyte * 16).from_buffer_copy(uuid.UUID("618736e0-3c3d-11cf-810c-00aa00389b71").bytes_le)


def checked(result):
    assert result >= 0, "MSAA HRESULT %#x" % (result & 0xffffffff)


def child_id(value=0):
    child = Variant()
    child.type, child.value.number = 3, value
    return child


class Accessible:
    def __init__(self, pointer):
        self.pointer = pointer

    def call(self, slot, args, *values):
        table = c.cast(self.pointer, c.POINTER(c.POINTER(c.c_void_p))).contents
        method = c.WINFUNCTYPE(c.c_long, c.c_void_p, *args)(table[slot])
        return method(self.pointer, *values)

    def close(self):
        if self.pointer:
            self.call(2, ())
            self.pointer = None

    def string(self, slot):
        value = c.c_void_p()
        checked(self.call(slot, (Variant, c.POINTER(c.c_void_p)), child_id(), c.byref(value)))
        try:
            return c.wstring_at(value) if value.value else ""
        finally:
            free_string(value)

    @property
    def name(self):
        return self.string(10)

    def number(self, slot):
        value = Variant()
        checked(self.call(slot, (Variant, c.POINTER(Variant)), child_id(), c.byref(value)))
        try:
            assert value.type == 3
            return value.value.number
        finally:
            clear_variant(c.byref(value))

    @property
    def role(self):
        return self.number(13)

    @property
    def state(self):
        return self.number(14)

    def action(self):
        checked(self.call(25, (Variant,), child_id()))

    def location(self):
        values = [c.c_long() for _ in range(4)]
        checked(self.call(22, (c.POINTER(c.c_long),) * 4 + (Variant,),
                          *(c.byref(v) for v in values), child_id()))
        return tuple(v.value for v in values)

    @contextmanager
    def children(self):
        count = c.c_long()
        checked(self.call(8, (c.POINTER(c.c_long),), c.byref(count)))
        values = (Variant * count.value)()
        fetched = c.c_long()
        nodes = []
        try:
            if count.value:
                checked(accessible_children(self.pointer, 0, count, values, c.byref(fetched)))
                assert fetched.value == count.value
            for value in values:
                assert value.type == 9  # VT_DISPATCH: full accessible child objects
                raw = Accessible(value.value.pointer)
                pointer = c.c_void_p()
                checked(raw.call(0, (c.c_void_p, c.POINTER(c.c_void_p)), IID, c.byref(pointer)))
                nodes.append(Accessible(pointer))
            yield nodes
        finally:
            for node in nodes:
                node.close()
            for value in values:
                clear_variant(c.byref(value))


@contextmanager
def client(hwnd):
    pythoncom.CoInitialize()
    root = None
    try:
        pointer = c.c_void_p()
        checked(accessible_window(hwnd, 0xfffffffc, IID, c.byref(pointer)))
        root = Accessible(pointer)
        yield root
    finally:
        if root:
            root.close()
        pythoncom.CoUninitialize()
