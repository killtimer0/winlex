"""Task Scheduler v1 name calls without pywin32's unlocked PyMem_Free."""
import ctypes as c
from ctypes import wintypes as w

import pythoncom
from comtypes import COMMETHOD, GUID, HRESULT, IUnknown, CoCreateInstance
from win32com.taskscheduler import taskscheduler as t


class ITaskScheduler(IUnknown):
    _iid_ = GUID(str(t.IID_ITaskScheduler))
    _methods_ = [
        COMMETHOD([], HRESULT, 'SetTargetComputer', (['in'], w.LPCWSTR, 'computer')),
        COMMETHOD([], HRESULT, 'GetTargetComputer', (['out'], c.POINTER(w.LPWSTR), 'computer')),
        COMMETHOD([], HRESULT, 'Enum', (['out'], c.POINTER(c.c_void_p), 'items')),
        COMMETHOD([], HRESULT, 'Activate', (['in'], w.LPCWSTR, 'name'),
                  (['in'], c.POINTER(GUID), 'iid'),
                  (['out'], c.POINTER(c.POINTER(IUnknown)), 'task')),
        COMMETHOD([], HRESULT, 'Delete', (['in'], w.LPCWSTR, 'name')),
        COMMETHOD([], HRESULT, 'NewWorkItem', (['in'], w.LPCWSTR, 'name'),
                  (['in'], c.POINTER(GUID), 'clsid'),
                  (['in'], c.POINTER(GUID), 'iid'),
                  (['out'], c.POINTER(c.POINTER(IUnknown)), 'task')),
    ]


def new_work_item(name):
    scheduler = CoCreateInstance(GUID(str(t.CLSID_CTaskScheduler)), interface=ITaskScheduler)
    task = scheduler.NewWorkItem(name, GUID(str(t.CLSID_CTask)), GUID(str(t.IID_ITask)))
    # ObjectFromAddress queries its own reference; comtypes retains ownership
    # of its original pointer until the local variable is released.
    return pythoncom.ObjectFromAddress(c.cast(task, c.c_void_p).value, t.IID_ITask)


def delete_work_item(name):
    scheduler = CoCreateInstance(GUID(str(t.CLSID_CTaskScheduler)), interface=ITaskScheduler)
    scheduler.Delete(name)
