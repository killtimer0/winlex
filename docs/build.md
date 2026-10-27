# 构建与验证

## 环境与依赖

- Windows 开发机，Rust MSVC 工具链（已验证 Rust 1.99.1）。
- Visual Studio C++ 工具和 Windows SDK。
- Python 3；下载构建依赖另需 PATH 中的 `7z` 或 `7zz`。
- 独立读取器验证需 .NET SDK 10；ABI 验证另需 Clang。

依赖版本和 SHA256 记录在 `inputs/*.json`。下载脚本使用标准 `HTTPS_PROXY` 环境变量：

```powershell
python scripts/prepare-inputs.py --build-deps
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
```

WinMD 写入 `inputs/Windows.Win32.winmd`，链接依赖写入 `tmp/deps/`。

## 构建程序

在 MSVC 开发终端中执行；`$link` 须指向匹配架构的 Microsoft `link.exe`：

```powershell
$link = Join-Path $env:VCToolsInstallDir 'bin/Hostx64/x86/link.exe'
./scripts/build.ps1 -Architecture x86 -LinkExe $link `
  -YyThunksObject tmp/deps/yy-thunks-1.2.3/x86/YY_Thunks_for_WinXP.obj `
  -VcLtlRoot tmp/deps/vc-ltl-5.3.1/sdk
```

x64 构建将架构、链接器和 OBJ 路径改为 x64。
脚本自动查找 Windows SDK 的 Microsoft `rc.exe`，也可用 `-RcExe PATH` 固定版本。
GUI 菜单、字符串、版本信息和清单从 `crates/winlex-gui/res/gui.rc` 编译并嵌入。
产物位于 `target/<Rust target>/release/`：`winlex.exe` 为命令行程序，`winlex-gui.exe` 为图形界面程序。
各自的 `.build.json` 记录构建版本和哈希。CLI 查询另需所需 WinMD。

## 系统兼容性

| 程序架构 | 最低系统 | Rust target |
|---|---|---|
| x86 | XP RTM 5.1.2600.0 | i686-pc-windows-msvc |
| x64 | XP x64 / Server 2003，5.2.3790 系列 | x86_64-pc-windows-msvc |

YY-Thunks 提供系统 API 回退，VC-LTL5 复用系统 `msvcrt.dll`。
`+crt-static` 选择由 VC-LTL 提供实现的 /MT 库。

## 测试与独立验证

运行库测试和开发用 CLI：

```powershell
cargo test -p winlex-core -p winlex-cli --release --locked
cargo build -p winlex-cli --example host-cli --release --locked
cargo fmt --all --check
cargo clippy -p winlex-core -p winlex-cli --all-targets --locked -- -D warnings
python tests/metadata/run.py
```

`tests/metadata/run.py` 比较 Rust 读取结果与 .NET `System.Reflection.Metadata`，并验证布局、泛型及跨源引用样例。报告写入 `tmp/verify/`。

GUI 使用独立 pytest 用例及公共 native/session 支持层，默认运行在隔离桌面：

```powershell
cargo clippy -p winlex-gui --all-targets --locked -- -D warnings
uv venv tmp/gui-tests/venv
uv pip install --python tmp/gui-tests/venv/Scripts/python.exe -r tests/gui/requirements.txt
tmp/gui-tests/venv/Scripts/python.exe tests/gui/run.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe --dpi pmv2 -q
tmp/gui-tests/venv/Scripts/python.exe tests/gui/run.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe --dpi pmv1 --language zh-CN -q
python tests/gui/resources.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe
```

`--dpi` 可改为 `pmv1`、`system`、`unaware` 或 `auto`。`--desktop current` 使用当前桌面。
`--language/--dpi` 属于测试参数，通过独立配置根及 `WINLEX_DPI` 启动 EXE。
结果在 `tmp/gui-tests/<run-id>/`：JUnit、环境/输入哈希、窗口树、失败截图、退出码和清理记录。
Runner 管理测试进程和隔离资源，退出及清理结果记录在 `run.json`。
消息测试通过发送窗口消息验证控件行为。实际键鼠专项使用 `--desktop current --real-input -k real_search`，要求解锁的活动输入桌面并取得前台；无法满足条件时报告失败。
Python 3.8+ 依赖锁定在 `tests/gui/requirements.txt`；可选新 API 按能力探测。XP RTM 解释器/系统支持仍须实机验证。

### 本机/SSH 调度

目标配置参考 `tests/gui/target.example.json`，实际主机配置放 `tmp/`。
`tests/gui/remote.py` 支持 `deploy/submit/status/cancel/collect/capabilities`，`local` 与 `ssh` 使用相同文件队列。
目标端先部署仓库、解释器和已记录哈希的输入，再从已登录交互会话启动：

```powershell
python tests/gui/remote.py worker --root C:/WinlexTests/runs
python tests/gui/remote.py --config tmp/target.json submit --exe C:/WinlexTests/winlex-gui.exe --winmd C:/WinlexTests/Windows.Win32.winmd -q
python tests/gui/remote.py --config tmp/target.json status --run-id RUN_ID
python tests/gui/remote.py --config tmp/target.json cancel --run-id RUN_ID
python tests/gui/remote.py --config tmp/target.json collect --run-id RUN_ID
```

SSH 仅操作队列及传输，GUI 由交互 worker 启动。收集 ZIP 默认回收到 `tmp/remote-tests/`。
验证范围分别记录：本机消息与 DPI、远程交互会话、实际键鼠输入，以及目标系统兼容性。XP RTM 兼容性需要在对应系统上进行实机验证。

可选的 SDK ABI 检查使用真实 SDK 声明验证布局、COM 槽位及位域初始化字节：

```powershell
$env:WINLEX_MSVC_ROOT = $env:VCToolsInstallDir
$env:WINLEX_SDK_INCLUDE = Join-Path $env:WindowsSdkDir "Include/$($env:WindowsSDKVersion.TrimEnd('\'))"
$env:WINLEX_CLANG = (Get-Command clang).Source
python tests/metadata/run.py --abi
```

Clang 验证 x86/x64/ARM64；MSVC 验证已安装的目标编译器。`WINLEX_GATE_CLI` 可指定另一个待测 exe。

验证真实 WinRT 元数据、SDK 头文件槽位和已解包的 App SDK 组件：

```powershell
python tests/metadata/run.py --winrt-sdk "$env:WindowsSdkDir" --winrt-version 10.0.26100.0 --winrt-apps PATH_TO_EXTRACTED_PACKAGES
```

`--winrt-apps` 可省略；指定时目录应包含 Foundation、WinUI 包的 `metadata/*.winmd`。`--winrt-version` 使用本机 SDK 版本。
