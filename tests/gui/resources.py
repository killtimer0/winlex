"""Inspect the built GUI's native resources, independent of its Rust loader."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import re
import struct
import tomllib
import xml.etree.ElementTree as ET


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
        decoded = {}
        for block in sorted({value // 16 + 1 for value in strings.values()}):
            assert languages(6, block) == [0x409, 0x804], "string languages differ"
            for language in (0x409, 0x804):
                data = resource(6, block, language)
                offset = 0
                for index in range(16):
                    count, = struct.unpack_from("<H", data, offset)
                    offset += 2
                    value = data[offset:offset + count * 2].decode("utf-16-le")
                    offset += count * 2
                    decoded[language, (block - 1) * 16 + index] = value
        for name, value in strings.items():
            en, zh = decoded[0x409, value], decoded[0x804, value]
            assert en and zh, f"untranslated resource {name}"
            assert sorted(re.findall(r"\{\w+\}", en)) == sorted(re.findall(r"\{\w+\}", zh)), name
        assert languages(4, ids["IDR_MAIN_MENU"]) == [0x409, 0x804]
        assert languages(9, ids["IDR_ACCELERATORS"]) == [0]
        accelerators = list(struct.iter_unpack("<4H", resource(9, ids["IDR_ACCELERATORS"], 0)))
        assert {(flags & 0x7f, key, command) for flags, key, command, _ in accelerators} == {
            (9, ord('L'), ids['FOCUS_SEARCH']),
            (9, 0x0D, ids['OPEN_NEW']),
            (1, 0x75, ids['FOCUS_NEXT_REGION'])}
        assert languages(24, 1) == [0], "manifest must be language neutral"
        manifest = ET.fromstring(resource(24, 1, 0))
        tags = [node.tag.rsplit("}", 1)[-1] for node in manifest.iter()]
        assert "dpiAware" not in tags and "dpiAwareness" not in tags, "manifest overrides runtime DPI choice"
        assert any(node.attrib.get("level") == "asInvoker" for node in manifest.iter())
        assert any(node.attrib.get("name") == "Microsoft.Windows.Common-Controls" and node.attrib.get("version") == "6.0.0.0" for node in manifest.iter())
        assert languages(16, 1) == [0]
        version = resource(16, 1, 0)
        signature = version.index(struct.pack("<I", 0xFEEF04BD))
        fields = struct.unpack_from("<13I", version, signature)
        actual = [fields[2] >> 16, fields[2] & 0xFFFF, fields[3] >> 16, fields[3] & 0xFFFF]
        expected = list(map(int, tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"].split("."))) + [0]
        assert actual == expected and fields[4:6] == fields[2:4]
        args.artifacts.mkdir(parents=True, exist_ok=True)
        (args.artifacts / "resources.json").write_text(json.dumps({"exe": str(args.exe), "strings_per_language": len(strings), "languages": ["en-US", "zh-CN"], "version": actual}, indent=2), encoding="utf-8")
        print(f"PASS resources: {len(strings)} strings per language, bilingual menu, manifest, version {actual}")
    finally:
        free(module)


if __name__ == "__main__":
    main()
