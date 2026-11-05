"""Inspect the built GUI's native resources, independent of its Rust loader."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import re
import struct
import xml.etree.ElementTree as ET

from support.languages import LANGUAGES


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--artifacts", type=Path, default=Path("tmp/gui-resources"))
    args = parser.parse_args()
    kernel = c.WinDLL("kernel32", use_last_error=True)

    def api(name, result, *parameters):
        fn = getattr(kernel, name)
        fn.restype, fn.argtypes = result, parameters
        return fn

    load = api("LoadLibraryExW", c.c_void_p, w.LPCWSTR, c.c_void_p, w.DWORD)
    free = api("FreeLibrary", w.BOOL, c.c_void_p)
    find = api("FindResourceExW", c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p, w.WORD)
    size = api("SizeofResource", w.DWORD, c.c_void_p, c.c_void_p)
    load_resource = api("LoadResource", c.c_void_p, c.c_void_p, c.c_void_p)
    lock = api("LockResource", c.c_void_p, c.c_void_p)
    language_callback = c.WINFUNCTYPE(w.BOOL, c.c_void_p, c.c_void_p, c.c_void_p, w.WORD, c.c_ssize_t)
    enum_languages = api("EnumResourceLanguagesW", w.BOOL, c.c_void_p, c.c_void_p, c.c_void_p, language_callback, c.c_ssize_t)
    module = load(str(args.exe.resolve()), None, 2)  # LOAD_LIBRARY_AS_DATAFILE, works across bitness.
    assert module, c.WinError(c.get_last_error())

    def languages(kind, name):
        values = []
        callback = language_callback(lambda _m, _t, _n, lang, _p: values.append(lang) or True)
        assert enum_languages(module, kind, name, callback, 0)
        return sorted(values)

    def resource(kind, name, language):
        info = find(module, kind, name, language)
        assert info, f"resource {kind}/{name}/{language} missing"
        pointer = lock(load_resource(module, info))
        assert pointer
        return c.string_at(pointer, size(module, info))

    try:
        root = Path(__file__).resolve().parents[2]
        header = (root / "crates/winlex-gui/res/resource.h").read_text(encoding="utf-8")
        ids = {name: int(value) for name, value in re.findall(r"^#define (\w+) (\d+)$", header, re.M)}
        strings = {name: value for name, value in ids.items() if name.startswith("IDS_")}
        assert len(ids) == len(set(ids.values())), "duplicate resource/command IDs"
        language_ids = sorted(language.langid for language in LANGUAGES)
        decoded = {}
        for block in sorted({value // 16 + 1 for value in strings.values()}):
            assert languages(6, block) == language_ids, "string languages differ"
            for language in language_ids:
                data = resource(6, block, language)
                offset = 0
                for index in range(16):
                    count, = struct.unpack_from("<H", data, offset)
                    offset += 2
                    value = data[offset:offset + count * 2].decode("utf-16-le")
                    offset += count * 2
                    decoded[language, (block - 1) * 16 + index] = value
        for name, value in strings.items():
            baseline = decoded[LANGUAGES[0].langid, value]
            for language in language_ids:
                text = decoded[language, value]
                assert text, f"untranslated resource {name}/{language:#06x}"
                assert sorted(re.findall(r"\{\w+\}", baseline)) == sorted(re.findall(r"\{\w+\}", text)), (name, language)
        assert languages(4, ids["IDR_MAIN_MENU"]) == language_ids
        for name in ("IDD_SETTINGS_GENERAL", "IDD_SETTINGS_SEARCH"):
            assert languages(5, ids[name]) == language_ids
            for language in language_ids:
                data = resource(5, ids[name], language)
                assert struct.unpack_from("<HH", data) == (1, 0xffff)
                style, = struct.unpack_from("<I", data, 12)
                assert style & 0x40000448 == 0x40000448
        assert languages(5, ids['IDD_ABOUT']) == language_ids
        assert languages(5, ids['IDD_SEARCH_HELP']) == language_ids
        for language in language_ids:
            data = resource(5, ids['IDD_ABOUT'], language)
            assert struct.unpack_from('<HH', data) == (1, 0xffff)
            assert 'https://' in decoded[language, ids['IDS_ABOUT_INFO']]
        assert languages(9, ids["IDR_ACCELERATORS"]) == [0]
        accelerators = list(struct.iter_unpack("<4H", resource(9, ids["IDR_ACCELERATORS"], 0)))
        assert {(flags & 0x7f, key, command) for flags, key, command, _ in accelerators} == {
            (9, ord('O'), ids['LOAD_METADATA']),
            (9, ord('L'), ids['FOCUS_SEARCH']),
            (9, ord('F'), ids['FIND_DOCUMENT']),
            (1, 0x72, ids['FIND_NEXT']),
            (5, 0x72, ids['FIND_PREVIOUS']),
            (9, 0x0D, ids['OPEN_NEW']),
            (1, 0x75, ids['FOCUS_NEXT_REGION']),
            (17, 0x25, ids['HISTORY_BACK']),
            (17, 0x27, ids['HISTORY_FORWARD'])}
        assert languages(24, 1) == [0], "manifest must be language neutral"
        manifest = ET.fromstring(resource(24, 1, 0))
        tags = [node.tag.rsplit("}", 1)[-1] for node in manifest.iter()]
        assert "dpiAware" not in tags and "dpiAwareness" not in tags, "manifest overrides runtime DPI choice"
        assert any(node.attrib.get("level") == "asInvoker" for node in manifest.iter())
        assert any(
            node.attrib.get("name") == "Microsoft.Windows.Common-Controls"
            and node.attrib.get("version") == "6.0.0.0"
            for node in manifest.iter()
        )
        assert languages(16, 1) == [0]
        version = resource(16, 1, 0)
        signature = version.index(struct.pack("<I", 0xFEEF04BD))
        fields = struct.unpack_from("<13I", version, signature)
        actual = [fields[2] >> 16, fields[2] & 0xFFFF, fields[3] >> 16, fields[3] & 0xFFFF]
        package = (root / "Cargo.toml").read_text(encoding="utf-8").split("[package]", 1)[1].split("\n[", 1)[0]
        expected = list(map(int, re.search(r'^version\s*=\s*"(\d+\.\d+\.\d+)"', package, re.M)[1].split("."))) + [0]
        assert actual == expected and fields[4:6] == fields[2:4]
        args.artifacts.mkdir(parents=True, exist_ok=True)
        (args.artifacts / "resources.json").write_text(json.dumps({
            "exe": str(args.exe),
            "strings_per_language": len(strings),
            "languages": [language.name for language in LANGUAGES],
            "version": actual,
        }, indent=2), encoding="utf-8")
        print(f"PASS resources: {len(strings)} strings per language, {len(LANGUAGES)} menu languages, manifest, version {actual}")
    finally:
        free(module)


if __name__ == "__main__":
    main()
