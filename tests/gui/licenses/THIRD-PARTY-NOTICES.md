# GUI test dependencies

## AutoItX 3.3.12.0

AutoItX is by Jonathan Bennett and the AutoIt Team. The GUI test backend uses
the DLLs bundled in the PyAutoIt 0.6.5 wheel. These binaries match the DLLs in
the official AutoIt 3.3.12.0 archive byte for byte.

AutoItX is governed by the separate AutoIt EULA, **not** the Python wrapper's
MIT license. The complete EULA from `AutoItX.chm`, `html/license.htm`, in the
official archive is reproduced as plain text in [AutoIt-EULA.txt](AutoIt-EULA.txt).
Keep this EULA and all copyright and trademark notices with redistributed DLLs.

Official source:
<https://www.autoitscript.com/autoit3/files/archive/autoit/autoit-v3.3.12.0.zip>

SHA256 (computed from the official archive and its DLLs):

| File | SHA256 |
| --- | --- |
| `autoit-v3.3.12.0.zip` | `67fb2dca6781e335c5c2a13290e29fcf98caa858ef7274c152574f2802a12c0f` |
| `autoit/lib/AutoItX3.dll` (x86) | `7c860b9ae4549ca2dd81c607bdb641ab2e38895aa5431a549df6c5da75b3dd89` |
| `autoit/lib/AutoItX3_x64.dll` (x64) | `1e4c494b81714633a7c067ff2820d1303cd83d9fd66886dab8bbe369e6514830` |

## PyAutoIt 0.6.5

Python wrapper by Jace Xu: <https://github.com/jacexh/pyautoit>.
The wrapper's MIT license is reproduced from the wheel's
`PyAutoIt-0.6.5.dist-info/LICENSE` in [PyAutoIt-MIT.txt](PyAutoIt-MIT.txt).

Distribution and published checksum:
<https://pypi.org/pypi/PyAutoIt/0.6.5/json>

| File | SHA256 |
| --- | --- |
| `PyAutoIt-0.6.5-py3-none-any.whl` | `00935e9a3d7b4afd1a4baa1d52ecaf1cd414bbc0332c7e0165101ef59ba055a8` |
