# 构建与验证

## 环境与依赖

- Windows 开发机，Rust MSVC 工具链。
- Git 用于记录提交与工作区状态；源码归档中构建时该信息留空。
- PowerShell 7.2+（`pwsh`）。
- Visual Studio C++ 工具和 Windows SDK。
- Python 3.12+ 和 `uv`。
- 独立读取器验证需 .NET SDK 10；ABI 验证另需 Clang。

链接依赖版本和 SHA256 记录在 `inputs/*.json`。下载脚本使用标准 `HTTPS_PROXY` 环境变量：

```powershell
uv sync --locked --python 3.12 --group tests --group gui --group media
./.venv/Scripts/Activate.ps1
python scripts/prepare-inputs.py
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
```

以上在开发机项目根目录创建并激活 `.venv`；后续 Python 命令均使用该环境。`tests`、`gui`、`media` 分别提供常规测试、GUI 测试和图片生成依赖，可按需选择。依赖定义及锁定版本见 `pyproject.toml` 和 `uv.lock`。

链接依赖写入 `tmp/deps/`。
可用 `--architecture x86|x64|arm64` 只准备目标架构需要的依赖；ARM64 不获取 YY-Thunks。
测试所需 Win32 输入的版本与哈希见 `tests/metadata/source.json`，将对应文件放入 `inputs/Windows.Win32.winmd`。

## 构建程序

先关闭待替换的 Winlex 实例，并清除已有的 `RUSTFLAGS`／`CARGO_ENCODED_RUSTFLAGS`。默认入口按目标组件选择 Visual Studio 并初始化匹配架构的开发环境，依赖路径取自固定清单：

```powershell
./scripts/build.ps1 -Architecture x64
```

也可在 MSVC 开发终端中显式指定工具路径；`$link` 须指向匹配架构的 Hostx64 Microsoft `link.exe`：

```powershell
$link = Join-Path $env:VCToolsInstallDir 'bin/Hostx64/x86/link.exe'
./scripts/build.ps1 -Architecture x86 -Configuration Debug -LinkExe $link
```

x64 构建将架构和链接器路径改为 x64。兼容库默认使用准备脚本的输出，也可用 `-YyThunksObject` 和 `-VcLtlRoot` 指定路径。
ARM64 需要 Rust 的 `aarch64-pc-windows-msvc` target 及 Visual Studio C++ ARM64 生成工具，使用 `Hostx64/arm64/link.exe`，不需要 YY-Thunks：

```powershell
$link = Join-Path $env:VCToolsInstallDir 'bin/Hostx64/arm64/link.exe'
./scripts/build.ps1 -Architecture arm64 -Configuration Debug -LinkExe $link
```
开发构建使用 `-Configuration Debug`；优化版本和分发版本分别使用 `Release` 和 `Dist`。
脚本使用所选 SDK 的 Microsoft `rc.exe`，也可用 `-RcExe PATH` 指定；Dist 不回退搜索其他 SDK。输出目录可用 `-TargetDir PATH` 指定，否则遵循 Cargo 配置。
构建入口在编译前校验清单中的兼容库与 SHA-256，并记录源码提交、工作区状态、目标策略、实际 Rust／Cargo／原生工具版本及依赖。Rust 使用当前工具链。
手动工作流、分发打包及上传前检查见[手动构建与交付](release.md)。
构建结束后显示 `winlex.exe`、`winlex-gui.exe` 的实际路径；对应的 `.build.json` 可用于生成消息资源 fixture 和验证打包产物。默认输出位于标准 `target/` 目录。

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

`inputs/targets.json` 集中定义各架构的构建与兼容策略，供原生链接、构建记录和打包校验共用。

| 程序架构 | 最低系统 | Rust target |
|---|---|---|
| x86 | XP RTM 5.1.2600.0 | i686-pc-windows-msvc |
| x64 | XP x64 / Server 2003，5.2.3790 系列 | x86_64-pc-windows-msvc |
| ARM64 | Windows 10 1809，10.0.17763.0（构建目标基线） | aarch64-pc-windows-msvc |

所有架构均通过 `+crt-static` 选择 VC-LTL5 替换库。x86／x64 使用系统 `msvcrt.dll` 并链接 YY-Thunks XP 对象；ARM64 使用系统 `ucrtbase.dll`。

## README 图片

在已解锁的当前桌面生成主图和操作 GIF（录制期间会接管键鼠）：

```powershell
python scripts/readme-media.py --exe target/x86_64-pc-windows-msvc/debug/winlex-gui.exe
```

图片写入 `docs/images/`，原始截图保留在 `tmp/readme-media/`。输入与输出选项见 `--help`。

## 测试与独立验证

运行库测试和开发用 CLI：

```powershell
cargo test -p winlex-core -p winlex-format -p winlex-cli --locked
cargo build -p winlex-cli --example host-cli --locked
cargo fmt --all --check
cargo clippy -p winlex-core -p winlex-format -p winlex-cli --all-targets --locked -- -D warnings
python tests/metadata/run.py
python -m unittest discover -s tests/metadata -p test_artifacts.py
python -m unittest discover -s tests/differential -v
python -m unittest discover -s tests/scripts -v
```

`tests/metadata/run.py` 验证 CLI、布局、泛型及跨源引用样例。报告写入 `tmp/verify/`。

对拍本地目录中的 WinMD：

```powershell
python tests/differential/run.py inputs
```

递归比较目录内的 `.winmd`，结果写入 `tmp/differential/`。查看 `report.json` 中的结果、比较范围及差异文件路径；失败返回非零退出码。选项见 `python tests/differential/run.py --help`。

打包契约测试默认读取标准输出目录下的 Debug 构建记录；可用 `WINLEX_PACKAGE_X86_BUILD_DIR`／`WINLEX_PACKAGE_X64_BUILD_DIR`／`WINLEX_PACKAGE_ARM64_BUILD_DIR` 指定其他目录，缺少记录时跳过打包用例。工作流 YAML 验证使用 `tests` 依赖组中的 PyYAML。
`WINLEX_PACKAGE_REQUIRE_ARCH` 指定必须验证的架构，缺少该架构的记录将失败；`WINLEX_TEST_ARCH` 为构建入口契约测试选择目标，默认 x64。
默认使用 Debug 和 Cargo 的 target、输出目录配置；可用 `--target`、`--target-dir`、`--profile` 显式选择。
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

GUI 测试使用 pytest；普通契约测试默认在独立的非活动桌面执行 UI Automation：

```powershell
cargo clippy -p winlex-gui --all-targets --locked -- -D warnings
python tests/gui/run.py --exe "$guiExe" --dpi pmv2 -q
python tests/gui/run.py --exe "$guiExe" --dpi pmv1 --language zh-CN -q
python tests/gui/run.py --desktop current --real-input --exe "$guiExe" -q
python tests/gui/resources.py --exe "$guiExe"
```

`$guiExe` 为构建显示的 GUI 完整路径。`--dpi` 可改为 `pmv1`、`system`、`unaware` 或 `auto`。默认使用 `--desktop isolated`；也可使用 `--desktop current` 在当前桌面运行契约测试。
结果写入 `tmp/gui-tests/<run-id>/`，包括 JUnit、失败截图和 `run.json`；`progress.jsonl` 即时记录当前用例及阶段，`timings.jsonl` 记录 setup、call、teardown 耗时，整批中断后仍可查看已完成阶段。
交互与屏幕呈现测试使用 `--desktop current --real-input` 选择；不指定 `--real-input` 时运行其余用例。交互测试需要已解锁的独占活动桌面，运行期间不要操作键鼠。
XP 目标执行端需有兼容 XP 的 Python 3.8 和 UI Automation 组件；通过 `python -m pip install -r tests/gui/requirements.lock` 安装目标端依赖，不使用开发机的 `.venv` 或 uv 锁文件。
`tests/gui/requirements.txt` 定义目标端直接依赖，`requirements.lock` 固定其 Python 3.8 Windows 传递依赖。在开发机更新清单后重新生成：

```powershell
uv pip compile tests/gui/requirements.txt --python .venv/Scripts/python.exe --python-version 3.8 --python-platform windows --no-python-downloads --no-annotate --output-file tests/gui/requirements.lock
```

消息资源等可选 fixture 的部署要求见 [测试目录说明](../tests/README.md)。

### 本机/SSH 调度

目标配置统一放在 `tests/gui/config/<target>.json`，例如 `tests/gui/config/xp.json`、`tests/gui/config/win10.json`；这些本机配置被 Git 忽略，不随克隆提供。新增目标时复制模板，再手动修改：

```powershell
Copy-Item tests/gui/config/target.example.json tests/gui/config/win10.json
```

SSH 配置字段如下：

| 字段 | 含义 |
| --- | --- |
| `transport` | SSH 目标使用 `ssh`；本机队列使用 `local`，只需另设 `root` |
| `host` | SSH 地址，可写为 `user@hostname`，或使用调度机 SSH 配置中的别名 |
| `key_file` | 调度机上的 SSH 私钥路径；支持 `~`，省略时使用 SSH 默认认证配置 |
| `port` | SSH 端口，省略时为 22 |
| `python` | 目标机上已安装测试依赖的 Python 解释器完整路径 |
| `script` | 目标机上部署的 `tests/gui/remote.py` 完整路径 |
| `root` | 目标机上的任务队列与结果目录；本机模式下为调度机目录 |
| `sftp_drive_root` | SFTP 盘符映射前缀；Bitvise 的 `/C/...` 映射填 `/`，使用原生 Windows 路径的服务省略此字段 |

JSON 中的反斜杠须写成 `\\`，路径也可使用 `/`。修改配置不会移动目标端文件；`script` 和 `root` 应与部署位置及 worker 启动参数一致。
目标端部署 `tests/gui/`、被测 EXE 和 WinMD 输入，保留 `tests/gui` 目录结构。配置的目标 Python 应已具备测试依赖。以下操作从调度机执行：

```powershell
python tests/gui/remote.py --config tests/gui/config/win10.json deploy --source tests/gui --destination C:/WinlexTests/tests
python tests/gui/remote.py --config tests/gui/config/win10.json start-worker
python tests/gui/remote.py --config tests/gui/config/win10.json capabilities
python tests/gui/remote.py --config tests/gui/config/win10.json submit --exe C:/WinlexTests/winlex-gui.exe --winmd C:/WinlexTests/Windows.Win32.winmd -q
python tests/gui/remote.py --config tests/gui/config/win10.json status --run-id RUN_ID
python tests/gui/remote.py --config tests/gui/config/win10.json watch --run-id RUN_ID --slow-after 15 --timeout 60
python tests/gui/remote.py --config tests/gui/config/win10.json cancel --run-id RUN_ID
python tests/gui/remote.py --config tests/gui/config/win10.json collect --run-id RUN_ID
```

`start-worker` 通过任务计划程序请求在当前用户已登录的交互会话中启动 worker，使用配置指定的 Python；`capabilities` 显示解释器路径、桌面和心跳。修改配置并完成部署后，用 `capabilities` 确认 `worker_available` 为 `true`，并核对 `executable`、`desktop` 和 `heartbeat_age`。需要该用户已登录以及创建计划任务的权限。也可在目标交互桌面手动执行 `python tests/gui/remote.py worker --root C:/WinlexTests/runs`。远程提交默认使用独立桌面；交互测试期间该会话须解锁且由测试独占，同一桌面只运行一个 worker。
`deploy` 的目标父目录须已存在。SSH 负责调度和传输，GUI 由交互 worker 启动。收集 ZIP 默认回收到 `tmp/remote-tests/`。选择交互测试可在提交时增加 `--desktop current --real-input -k real_search_keyboard`。

`watch` 在当前阶段达到 `--slow-after` 秒、运行结束或观察时间到期时返回，不会终止测试。慢阶段预警包含用例、setup／call／teardown 阶段及耗时，可结合运行日志中的线程栈及时诊断。

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
