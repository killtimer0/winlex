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
产物位于 `target/<Rust target>/release/`：`winlex.exe` 为 CLI，`winlex-gui.exe` 为 GUI 骨架。
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
python verify/run.py
```

`verify/run.py` 比较 Rust 读取结果与 .NET `System.Reflection.Metadata`，并验证布局、泛型及跨源引用样例。报告写入 `tmp/verify/`。

GUI 骨架检查使用 Windows 10 1703+ API，默认在独立桌面运行；双屏 DPI 不同时检查跨屏布局：

```powershell
cargo clippy -p winlex-gui --all-targets --locked -- -D warnings
python verify/gui.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe --dpi pmv2
python verify/gui.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe --dpi pmv1 --lang zh-CN
python verify/gui_resources.py --exe target/i686-pc-windows-msvc/release/winlex-gui.exe
```

`--dpi` 可改为 `pmv1`、`system`、`unaware` 或 `auto`。`--desktop current` 使用当前桌面。
GUI 检查默认使用英文，`--lang zh-CN` 检查中文文案及选项映射。资源检查验证双语字符串、菜单、清单和版本号。
检查通过窗口消息操作控件，不替代真实鼠标输入测试。诊断写入 `tmp/gui-check/`。

可选的 SDK ABI 检查使用真实 SDK 声明验证布局、COM 槽位及位域初始化字节：

```powershell
$env:WINLEX_MSVC_ROOT = $env:VCToolsInstallDir
$env:WINLEX_SDK_INCLUDE = Join-Path $env:WindowsSdkDir "Include/$($env:WindowsSDKVersion.TrimEnd('\'))"
$env:WINLEX_CLANG = (Get-Command clang).Source
python verify/run.py --abi
```

Clang 验证 x86/x64/ARM64；MSVC 验证已安装的目标编译器。`WINLEX_GATE_CLI` 可指定另一个待测 exe。

验证真实 WinRT 元数据、SDK 头文件槽位和已解包的 App SDK 组件：

```powershell
python verify/run.py --winrt-sdk "$env:WindowsSdkDir" --winrt-version 10.0.26100.0 --winrt-apps PATH_TO_EXTRACTED_PACKAGES
```

`--winrt-apps` 可省略；指定时目录应包含 Foundation、WinUI 包的 `metadata/*.winmd`。`--winrt-version` 使用本机 SDK 版本。
