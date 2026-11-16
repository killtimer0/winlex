# 构建与验证

## 环境与依赖

- Windows 开发机，Rust MSVC 工具链。
- Git（可选，用于记录构建来源）。
- PowerShell 7.2+（`pwsh`）。
- Visual Studio C++ 工具和 Windows SDK。
- Python 3.12+ 和 `uv`。
- 独立读取器验证需 `global.json` 指定的 .NET SDK；ABI 验证另需 Clang。

链接依赖版本和 SHA256 记录在 `inputs/*.json`。下载脚本使用标准 `HTTPS_PROXY` 环境变量：

```powershell
uv sync --locked --python 3.12 --group tests --group gui
./.venv/Scripts/Activate.ps1
python scripts/prepare-inputs.py
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
```

所有命令从项目根目录运行。以上创建并激活开发机 `.venv`，后续 Python 命令均使用该环境；依赖组可按需选择，定义及锁定版本见 `pyproject.toml` 和 `uv.lock`。

链接依赖写入 `tmp/deps/`。
可用 `--architecture x86|x64|arm64` 只准备目标架构需要的依赖；ARM64 不获取 YY-Thunks。
测试所需 Win32 输入的版本与哈希见 `tests/metadata/source.json`，运行 `python tests/metadata/prepare_source.py` 下载并校验到 `inputs/Windows.Win32.winmd`。

## 构建程序

先关闭待替换的 Winlex 实例，并清除已有的 `RUSTFLAGS`／`CARGO_ENCODED_RUSTFLAGS`。默认入口按目标组件选择 Visual Studio 并初始化匹配架构的开发环境，依赖路径取自固定清单：

```powershell
./scripts/build.ps1 -Architecture x64
```

`-Architecture` 可选 `x86`、`x64`、`arm64`；ARM64 另需安装 Rust 的 `aarch64-pc-windows-msvc` target 和 Visual Studio C++ ARM64 生成工具，不需要 YY-Thunks。
默认使用 Debug 和当前 Rust 工具链；优化版本和分发版本分别使用 `-Configuration Release` 和 `Dist`。
输出目录可用 `-TargetDir PATH` 指定，否则遵循 Cargo 配置。
在 MSVC 开发终端中可用 `-LinkExe PATH` 指定匹配架构的 `Hostx64/<arch>/link.exe`，用 `-RcExe PATH` 指定 Microsoft `rc.exe`；兼容库路径可用 `-YyThunksObject` 和 `-VcLtlRoot` 覆盖。

CI 工具版本见 [工作流](../.github/workflows/build.yml)，.NET SDK 以 `global.json` 为准。本地可设置 `WINLEX_SDK_VERSION` 强制选择 SDK，缺失或不符时报错；未设置时使用 Visual Studio 默认值。Dist 不回退搜索其他 SDK。
构建会校验兼容库哈希，显示 EXE 路径，并生成记录源码、工具和依赖的 `.build.json`，供 fixture 生成与[打包](release.md)使用。

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

构建目标的最低系统为 x86 的 XP RTM、x64 的 XP x64 / Server 2003，以及 ARM64 的 Windows 10 1809。精确版本号、Rust target 和链接策略见 [inputs/targets.json](../inputs/targets.json)。

兼容库与暗色实现的来源和许可见[第三方声明](../THIRD-PARTY-NOTICES.md)。暗色实现不依赖额外 DLL；外观可用范围见 [GUI 指南](gui.md#界面与设置)。

## 测试与独立验证

运行库测试、静态检查和独立验证：

```powershell
$env:WINLEX_TEST_WINMD = (Resolve-Path inputs/Windows.Win32.winmd).Path
cargo test -p winlex-core -p winlex-format -p winlex-cli --all-targets --locked
cargo fmt --all --check
cargo clippy -p winlex-core -p winlex-format -p winlex-cli --all-targets --locked -- -D warnings
python tests/metadata/run.py
python -m unittest discover -s tests/metadata -v
python -m unittest discover -s tests/differential -v
python -m unittest discover -s tests/scripts -v
```

`tests/metadata/run.py` 自动构建开发用 CLI，验证布局、泛型及跨源引用，报告写入 `tmp/verify/`。默认使用 Debug 和 Cargo 配置，可用 `--target`、`--target-dir`、`--profile` 覆盖。

Rust 测试统一通过 `WINLEX_TEST_WINMD` 读取 Win32 输入；直接执行测试时须设置为执行机上的路径，`tests/metadata/run.py` 会在校验输入后设置此变量。

GUI 行为测试使用下文的 pytest 入口；普通 Rust 测试不创建桌面窗口。

使用 CLR 参考程序集验证基础类型与 COM 结构体的原生布局：

```powershell
python tests/metadata/prepare_source.py --foundation-reference
python tests/metadata/foundation_gate.py --reference tmp/verify/reference/netstandard.dll
```

下载内容由[参考程序集清单](../tests/metadata/foundation-reference.json)锁定，仅供回归验证；`foundation_gate.py --inputs` 可指定 WinMD 或集合目录。

对拍本地目录中的 WinMD：

```powershell
python tests/differential/run.py inputs
```

递归比较目录内的 `.winmd`，结果写入 `tmp/differential/`。查看 `report.json` 中的结果、比较范围及差异文件路径；失败返回非零退出码。选项见 `python tests/differential/run.py --help`。

打包契约测试默认读取标准输出目录下的 Debug 构建记录；可用 `WINLEX_PACKAGE_X86_BUILD_DIR`／`WINLEX_PACKAGE_X64_BUILD_DIR`／`WINLEX_PACKAGE_ARM64_BUILD_DIR` 指定其他目录，缺少记录时跳过打包用例。工作流 YAML 验证使用 `tests` 依赖组中的 PyYAML。
`WINLEX_PACKAGE_REQUIRE_ARCH` 指定必须验证的架构，缺少该架构的记录将失败；`WINLEX_TEST_ARCH` 为构建入口契约测试选择目标，默认 x64。
各 gate 也可独立运行，默认读取 `tmp/verify/artifacts.json` 中的可执行文件路径；用 `--artifacts PATH` 更改记录位置后，通过 `WINLEX_GATE_ARTIFACTS` 指定该记录。

```powershell
python tests/metadata/fixture_gate.py
python tests/metadata/multisource_gate.py
```

生成消息资源 fixture 时，传入双架构构建显示的实际 `.build.json` 路径：

```powershell
python tests/fixtures/build_message_fixtures.py --x86-build "$x86Cli.build.json" --x64-build "$x64Cli.build.json"
```

`$x86Cli` 和 `$x64Cli` 为对应 CLI 的完整路径；资源 DLL 写入 `tmp/message-fixtures/`。

### GUI 执行端准备

`tests/gui` 使用 pytest、AutoItX 和 pywin32。目标端准备 Python 3.8，解释器位数须与被测 GUI EXE 一致；XP 所需的兼容版解释器需自行准备。在该解释器中安装目标依赖：

```powershell
python -m pip install -r tests/gui/requirements.lock
```

目标环境独立于开发机 `.venv` 和 `uv.lock`。SSH 配置的解释器同目录须有 `pythonw.exe`，目标须提供 `taskkill.exe`。PyAutoIt wheel 所含 AutoItX DLL 的许可独立于 Python 包装器的 MIT 许可；分发时保留 [AutoIt EULA](../tests/gui/licenses/AutoIt-EULA.txt) 与[依赖声明](../tests/gui/licenses/THIRD-PARTY-NOTICES.md)。

目标用户须已登录，独占已解锁、无屏保的 `WinSta0/Default` 输入桌面。普通用例也会创建真实窗口，`--real-input` 另启用真实键盘场景。运行期间不要操作键鼠或切换会话。

直接依赖维护在 `tests/gui/requirements.txt`；修改后在开发机重新生成目标锁文件：

```powershell
uv pip compile tests/gui/requirements.txt --python .venv/Scripts/python.exe --python-version 3.8 --python-platform windows --no-python-downloads --no-annotate --output-file tests/gui/requirements.lock
```

### 本机/SSH 调度

GUI 测试默认提交远程目标。统一入口为 `tests/gui/run.py`，远程失败不会回退本机。调度机须提供 `ssh`、`sftp` 和可非交互使用的密钥认证，并预先确认主机密钥；目标用户须有目录读写和交互计划任务管理权限。

复制[配置模板](../tests/gui/config/target.example.json)到被 Git 忽略的目标配置：

```powershell
Copy-Item tests/gui/config/target.example.json tests/gui/config/target.json
```

字段以模板为准：`host` 为 SSH 地址或别名，`key_file` 为调度机私钥路径，`python` 和 `root` 为目标端绝对 Windows 路径。`root` 保存各轮部署与结果。`sftp_drive_root` 用于盘符映射：服务将 `C:/...` 映射为 `/C/...` 时填 `/`；直接接受 Windows 路径的服务省略此字段。

从开发机提交本机已有的 Debug EXE 和 WinMD，入口自动部署脚本、许可及输入：

```powershell
python tests/gui/run.py --config tests/gui/config/target.json --exe target/x86_64-pc-windows-msvc/debug/winlex-gui.exe --winmd inputs/Windows.Win32.winmd --output tmp/gui-target-run --timeout 600 -- --real-input -vv --durations=0
```

测试通过一次性交互计划任务启动：XP 使用旧版 Task Scheduler 接口，较新系统使用 `ONCE` 交互任务。每轮采用独立 UUID 目录及 `WinlexGui-<UUID>` 任务名，结束时删除任务、保留目标目录，无需常驻进程。

只有明确获准在本机运行时才使用：

```powershell
python tests/gui/run.py --local --allow-local-desktop --exe target/x86_64-pc-windows-msvc/debug/winlex-gui.exe --winmd inputs/Windows.Win32.winmd --output tmp/gui-local-run --timeout 600 -- --real-input -vv --durations=0
```

`--output` 是调度机上的不存在或空目录；`--exe`、`--winmd` 均为调度机输入路径。x86 验证改用 x86 EXE 和 x86 目标 Python。`--` 后传 pytest 参数，不需要真实输入时省略 `--real-input`，用 `-k` 或套件内测试节点缩小范围。入口参数见 `--help`，单例超时配置见 [`pytest.ini`](../tests/gui/pytest.ini)。

### 结果、超时与中断

终端在每个用例（含 teardown）结束后追加 `.`（通过）、`S`（跳过）或 `F`（任一阶段失败）。本机直接读取 `progress.txt`，SSH 随现有结果轮询增量读取；硬退出前未完成的用例不追加标记。进度不代表整轮完成，最终状态以 `result.json` 为准。

先检查输出目录的 `result.json`（退出码、超时及清理结果）和 `pytest.log`，pytest 报告为 `junit.xml`；硬超时可能来不及写出完整 JUnit。`input-hashes.json`、`execution-environment.json` 分别记录部署校验与环境，各用例目录保存进程和驱动记录。失败证据包括动作、窗口信息及 `screen.bmp` 截图；截图失败时查看 `screen-error.txt`。

单例调用超时会终止整个 pytest，不重试或续跑剩余用例。`--timeout` 限制整轮预算，SSH 模式包括部署与等待；收尾和回收有界执行。每例使用独立应用 Job 和私有注册表配置，执行器另行核对应用退出及配置清理。

执行框架的故障回归独立运行，不参与普通行为测试：

```powershell
python tests/gui/verify_faults.py --config tests/gui/config/target.json --exe target/x86_64-pc-windows-msvc/debug/winlex-gui.exe --winmd inputs/Windows.Win32.winmd --output tmp/gui-faults
```

入口逐项核对故障注入、预期退出及独立清理，全部通过后写出 `verification.json`。验收项目定义在 [`verify_faults.py`](../tests/gui/verify_faults.py)；单项注入的 `--fault-mode` 取值由 [`conftest.py`](../tests/gui/conftest.py) 注册。

Ctrl+C 尝试取消本轮并检查清理。出现 `RESULT UNKNOWN` 时保留打印出的目标 UUID 路径和任务名，不自动重跑。断线后通过 `scp` 或 `sftp` 取回同一 UUID 下的 `output/` 或已有 `artifacts.zip`，核对结果与清理状态。

自动删除任务失败时，确认本轮已停止后再删除确切任务名：较新系统使用 `schtasks /Delete /TN WinlexGui-<UUID> /F`，XP 使用旧版 Task Scheduler 管理界面或接口。删除任务不等于进程已退出；清理不明时保留原目录用于诊断。

### SDK ABI 与 WinRT

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
