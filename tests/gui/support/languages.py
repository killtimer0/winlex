"""Independent expectations for supported GUI languages."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Language:
    name: str
    langid: int
    label: str
    auto_label: str
    search_label: str


LANGUAGES = (
    Language("en-US", 0x0409, "English", "System default", "Search"),
    Language("zh-CN", 0x0804, "简体中文", "跟随系统", "搜索"),
)


def language(name):
    return next(option for option in LANGUAGES if option.name == name)
