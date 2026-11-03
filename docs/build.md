# 构建与验证

## 环境与依赖

- Windows 开发机，Rust MSVC 工具链。
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

从仓库根目录在 MSVC 开发终端中执行。先关闭待替换的 Winlex 实例，并清除已有的 `RUSTFLAGS`／`CARGO_ENCODED_RUSTFLAGS`；`$link` 须指向匹配架构的 Hostx64 Microsoft `link.exe`：

```powershell
$link = Join-Path $env:VCToolsInstallDir 'bin/Hostx64/x86/link.exe'
./scripts/build.ps1 -Architecture x86 -Configuration Debug -LinkExe $link `
  -YyThunksObject tmp/deps/yy-thunks-1.2.3/x86/YY_Thunks_for_WinXP.obj `
  -VcLtlRoot tmp/deps/vc-ltl-5.3.1/sdk
```

x64 构建将架构、链接器和 OBJ 路径改为 x64。
开发构建使用 `-Configuration Debug`；优化版本使用 `-Configuration Release`。
分发版本使用 `-Configuration Dist`。
脚本优先从开发环境的 Windows SDK 路径查找 Microsoft `rc.exe`，也可用 `-RcExe PATH` 指定。
输出目录遵循 Cargo 配置和 `CARGO_TARGET_DIR`，也可用 `-TargetDir PATH` 指定。
默认产物位于 `target/<Rust target>/{debug,release,dist}/`；构建结束后显示实际路径。
`winlex.exe` 为命令行程序，`winlex-gui.exe` 为图形界面程序。
各自的 `.build.json` 可供消息资源 fixture 构建使用。元数据查询另需 WinMD 文件。

## 更新图标资源

在项目根目录使用 Node.js 和 Windows PowerShell 运行：

```powershell
npm ci
npm run svg2ico
npm run svg2emf
```

程序图标源文件为 `crates/winlex-gui/res/appicon.svg`，ICO 输出到同目录。
Codicon SVG 与生成的 EMF+ 位于 `crates/winlex-gui/res/codicons/`。
生成资源随源文件一并提交，常规构建直接使用已生成的资源。

## 系统兼容性

| 程序架构 | 最低系统 | Rust target |
|---|---|---|
| x86 | XP RTM 5.1.2600.0 | i686-pc-windows-msvc |
| x64 | XP x64 / Server 2003，5.2.3790 系列 | x86_64-pc-windows-msvc |

兼容构建需要上述 YY-Thunks 和 VC-LTL5 依赖。

## 测试与独立验证

运行库测试和开发用 CLI：

```powershell
cargo test -p winlex-core -p winlex-format -p winlex-cli --locked
cargo build -p winlex-cli --example host-cli --locked
cargo fmt --all --check
cargo clippy -p winlex-core -p winlex-format -p winlex-cli --all-targets --locked -- -D warnings
python tests/metadata/run.py
python -m unittest discover -s tests/metadata -p test_artifacts.py
```

`tests/metadata/run.py` 比较 Rust 读取结果与 .NET `System.Reflection.Metadata`，并验证布局、泛型及跨源引用样例。报告写入 `tmp/verify/`。
默认使用 Debug，遵循 Cargo 的 target 和输出目录配置；可用 `--target`、`--target-dir "PATH WITH SPACES"`、`--profile release` 显式选择。
运行后，实际可执行文件路径保存在 `tmp/verify/artifacts.json`；可用 `--artifacts PATH` 更改位置。
各 gate 可独立运行，默认读取该记录；自定义记录通过 `WINLEX_GATE_ARTIFACTS` 指定。
`WINLEX_GATE_CLI`、`WINLEX_GATE_READER_FACTS`、`WINLEX_GATE_QUERY_AUDIT` 分别覆盖 CLI、读取器和 WinRT 审计程序路径，优先于记录中的值。

```powershell
python tests/metadata/reader_gate.py
python tests/metadata/fixture_gate.py
python tests/metadata/multisource_gate.py
```

生成消息资源 fixture 时，传入双架构构建显示的实际 `.build.json` 路径：

```powershell
python tests/fixtures/build_message_fixtures.py --x86-build "$x86Cli.build.json" --x64-build "$x64Cli.build.json"
```

`$x86Cli` 和 `$x64Cli` 为对应 CLI 的完整路径；资源 DLL 写入 `tmp/message-fixtures/`。

GUI 测试使用 pytest，默认运行在隔离桌面；以下命令另需安装 `uv`：

```powershell
cargo clippy -p winlex-gui --all-targets --locked -- -D warnings
uv venv tmp/gui-tests/venv
uv pip install --python tmp/gui-tests/venv/Scripts/python.exe -r tests/gui/requirements.txt
tmp/gui-tests/venv/Scripts/python.exe tests/gui/run.py --exe "$guiExe" --dpi pmv2 -q
tmp/gui-tests/venv/Scripts/python.exe tests/gui/run.py --exe "$guiExe" --dpi pmv1 --language zh-CN -q
python tests/gui/resources.py --exe "$guiExe"
```

`$guiExe` 为构建显示的 GUI 完整路径。`--dpi` 可改为 `pmv1`、`system`、`unaware` 或 `auto`。`--desktop current` 使用当前桌面。
结果写入 `tmp/gui-tests/<run-id>/`，包括 JUnit、失败截图和 `run.json`。
交互与屏幕呈现组使用 `--desktop current --real-input`，只运行标记为 `interactive` 的用例；可加 `-k real_search_keyboard` 等选择器。默认隔离桌面只运行其余 CLI／窗口契约用例。两组均可加 `--collect-only -q` 核对选择结果。
交互组要求专用 VM 中解锁的独占活动会话，并允许测试程序取得前台；测试期间不要操作键鼠。职责划分见 [测试目录说明](../tests/README.md)。
执行端使用与目标系统兼容的 Python 3.8+，通过 `tests/gui/requirements.txt` 安装依赖。
x64 EXE 需要 64 位执行端 Python；x86 EXE 可使用 32 位或 64 位执行端 Python。
消息资源等可选 fixture 的部署要求见 [测试目录说明](../tests/README.md)。

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

可选的 SDK ABI 检查使用真实 SDK 声明验证布局、COM 槽位及位域初始化字节：

```powershell
$env:WINLEX_MSVC_ROOT = $env:VCToolsInstallDir
$env:WINLEX_SDK_INCLUDE = Join-Path $env:WindowsSdkDir "Include/$($env:WindowsSDKVersion.TrimEnd('\'))"
$env:WINLEX_CLANG = (Get-Command clang).Source
python tests/metadata/run.py --abi
# 已有产物记录时也可单独运行
python tests/abi/run.py
```

Clang 验证 x86/x64/ARM64；MSVC 验证已安装的目标编译器。`WINLEX_GATE_CLI` 可指定另一个待测 exe。

验证真实 WinRT 元数据、SDK 头文件槽位和已解包的 App SDK 组件：

```powershell
python tests/metadata/run.py --winrt-sdk "$env:WindowsSdkDir" --winrt-version "$($env:WindowsSDKVersion.TrimEnd('\'))" --winrt-apps PATH_TO_EXTRACTED_PACKAGES
```

`--winrt-apps` 可省略；指定时目录应包含 Foundation、WinUI 包的 `metadata/*.winmd`。`--winrt-version` 使用本机 SDK 版本。
