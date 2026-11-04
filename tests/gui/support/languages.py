"""Independent expectations for supported GUI languages."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Language:
    name: str
    langid: int
    label: str
    auto_label: str
    search_label: str
    ok_label: str
    cancel_label: str
    category_labels: tuple


LANGUAGES = (
    Language("en-US", 0x0409, "English", "System default", "Search", "OK", "Cancel",
             ("&Types", "Functions/&Methods", "&Constants", "&Fields", "&Properties", "&Events", "Type &references")),
    Language("zh-CN", 0x0804, "简体中文", "跟随系统", "搜索", "确定", "取消",
             ("类型(&T)", "函数/方法(&M)", "常量(&C)", "字段(&F)", "属性(&P)", "事件(&E)", "类型引用(&R)")),
)


def language(name):
    return next(option for option in LANGUAGES if option.name == name)
