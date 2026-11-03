"""Build resource-only PE fixtures; does not run target executables."""
import json
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'tmp/message-fixtures'
OUT.mkdir(exist_ok=True)


def message_table(text):
    entries = []
    for value, unicode in [(text, True),
                           ('"0x%08lx" referenced "0x%08lx"; operation "%s".\r\n', True),
                           ('ANSI %08lx and %1!s! remain complete.\r\n', False)]:
        payload = (value + '\0').encode('utf-16le' if unicode else 'ascii')
        length = (4 + len(payload) + 3) & ~3
        entries.append(struct.pack('<HH', length, int(unicode)) + payload
                       + bytes(length - 4 - len(payload)))
    return struct.pack('<IIII', 1, 0x20000001, 0x20000003, 16) + b''.join(entries)


for lang, text in [('en', 'Fixture %1!s! message.\r\n'),
                   ('zh', '测试消息 %1!s!。\r\n')]:
    (OUT / (lang + '.bin')).write_bytes(message_table(text))
(OUT / 'messages.rc').write_text(
    'LANGUAGE 9, 1\n1 11 "en.bin"\nLANGUAGE 4, 2\n1 11 "zh.bin"\n',
    encoding='ascii')
(OUT / 'empty.rc').write_text('1 RCDATA { 1, 2, 3 }\n', encoding='ascii')
(OUT / 'neutral.rc').write_text('LANGUAGE 0, 0\n1 11 "en.bin"\n', encoding='ascii')
for arch, target in [('x86', 'i686-pc-windows-msvc'), ('x64', 'x86_64-pc-windows-msvc')]:
    build = json.loads((ROOT / 'target' / target / 'debug/winlex.exe.build.json').read_text())
    for stem in ['messages', 'empty', 'neutral']:
        res = OUT / (stem + '.res')
        subprocess.run([build['resource_compiler'], '/nologo', '/fo', str(res),
                        str(OUT / (stem + '.rc'))], cwd=OUT, check=True)
        subprocess.run([build['linker'], '/nologo', '/dll', '/noentry', '/nodefaultlib',
                        '/machine:' + arch, '/out:' + str(OUT / (stem + '-' + arch + '.dll')),
                        str(res)], cwd=OUT, check=True)
print(OUT)
