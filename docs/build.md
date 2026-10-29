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
默认构建 Release，使用 `-Configuration Debug` 构建开发版本。
脚本优先从开发环境的 Windows SDK 路径查找 Microsoft `rc.exe`，也可用 `-RcExe PATH` 指定。
GUI 菜单、字符串、版本信息和清单从 `crates/winlex-gui/res/gui.rc` 编译并嵌入。
输出目录遵循 Cargo 配置和 `CARGO_TARGET_DIR`，也可用 `-TargetDir PATH` 指定。
默认产物位于 `target/<Rust target>/{debug,release}/`；构建结束后显示实际路径。
`winlex.exe` 为命令行程序，`winlex-gui.exe` 为图形界面程序。
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
消息测试通过发送窗口消息验证控件行为。实际键鼠专项使用 `--desktop current --real-input -k real_search`，要求解锁的活动输入桌面并取得前台；无法满足条件时报告失败。字母输入专项会临时关闭搜索框的 IME，并在清理时恢复原状态。
执行端使用与目标系统兼容的 Python 3.8+ 和 pywin32。直接依赖列在 `tests/gui/requirements.txt`，按 Python 版本选择 pywin32：Python 3.8 使用 228，更新版本使用 310。可选系统 API 按能力探测。
`--real-input` 也使结果预览与双击测试使用真实鼠标输入，并在结束时恢复光标和前台窗口；这些测试须运行在活动输入桌面。
被测 EXE 可为 x86 或 x64，控件数据结构按 EXE 位数处理。x64 EXE 需要 64 位执行端 Python；x86 EXE 可使用 32 位或 64 位执行端 Python。元数据查询的架构选项独立于执行端和 EXE 位数。

### 本机/SSH 调度

目标配置参考 `tests/gui/target.example.json`，实际主机配置放 `tmp/`。
配置中的 `python` 指向目标机已有的系统解释器，`key_file` 指定 SSH 私钥，`port` 默认为 22。Bitvise 的 `/C/...` 路径映射使用 `sftp_drive_root: "/"`；使用原生 Windows 路径的 SFTP 服务可省略该字段。
目标端部署 `tests/gui/`、被测 EXE 和 WinMD 输入，保留 `tests/gui` 目录结构。系统 Python 应已具备测试依赖。以下操作从调度机执行：

```powershell
python tests/gui/remote.py --config tmp/target.json deploy --source tests/gui --destination C:/WinlexTests/tests
python tests/gui/remote.py --config tmp/target.json start-worker
python tests/gui/remote.py --config tmp/target.json capabilities
python tests/gui/remote.py --config tmp/target.json submit --exe C:/WinlexTests/winlex-gui.exe --winmd C:/WinlexTests/Windows.Win32.winmd -q
python tests/gui/remote.py --config tmp/target.json status --run-id RUN_ID
python tests/gui/remote.py --config tmp/target.json cancel --run-id RUN_ID
python tests/gui/remote.py --config tmp/target.json collect --run-id RUN_ID
```

`start-worker` 通过任务计划程序请求在当前用户已登录的交互会话中启动 worker，使用配置指定的 Python；`capabilities` 显示解释器路径、桌面和心跳。需要该用户已登录以及创建计划任务的权限。也可在目标交互桌面手动执行 `python tests/gui/remote.py worker --root C:/WinlexTests/runs`。同一桌面只运行一个 worker。
`deploy` 的目标父目录须已存在。SSH 负责调度和传输，GUI 由交互 worker 启动。收集 ZIP 默认回收到 `tmp/remote-tests/`。真实键鼠测试在提交时增加 `--desktop current --real-input -k real_search_keyboard`。
每轮结果记录目标系统、解释器与 EXE 位数、API 能力及跳过原因；本机消息与 DPI、远程交互会话、实际键鼠输入分别验证。

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
