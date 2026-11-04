# Winlex

离线 Windows API 参考查询工具，直接读取 WinMD 文件。

- 快速搜索名称、常量值和 GUID，或组合名称、字符串、数字范围等条件。
- 查看 API 参数、类型、回调及反向类型引用。
- 按 x86、x64、ARM64 ABI 计算结构布局、位域位置和 COM vtable 槽位。
- 支持文本和 JSON 输出。
- 通过独立错误码工具查询 Win32、HRESULT、NTSTATUS 和自定义模块消息，无需加载 WinMD。
- 可同时加载 SDK、WDK、WinRT 和 Windows App SDK 元数据，解析跨文件引用。
- 支持 WinRT/App SDK 声明、泛型、属性和事件查询，以及非泛型接口/委托槽位。

支持 Windows x86/x64，最低系统版本见[构建与验证](docs/build.md)。

## 使用

在命令前指定输入文件：

```powershell
winlex Windows.Win32.winmd search OPENFILE
winlex Windows.Win32.winmd search 0x80000000 --value
winlex Windows.Win32.winmd search CreateFileW --name
winlex Windows.Win32.winmd search "FF 0A" --guid
winlex Windows.Win32.winmd show CreateFileW --arch x64
winlex Windows.Win32.winmd layout OVERLAPPED --arch x64
winlex Windows.Win32.winmd vtable IStream --arch x64 --json
```

查询架构与程序自身架构无关。命令、输出及限制见 [CLI 文档](docs/cli.md)。

### 图形界面

`winlex-gui.exe` 提供原生 MDI 多文档界面，支持后台加载多个 WinMD、快速与条件搜索、来源管理和不同架构的声明比较。

```powershell
winlex-gui
winlex-gui Windows.Win32.winmd Windows.Wdk.winmd
```

通过“打开文件”（Ctrl+O）或拖放加载 WinMD。普通启动会恢复上次成功打开的来源集合；指定文件路径启动时使用临时会话。

输入查询后自动搜索，Enter 立即执行。右侧可选择快速搜索或条件搜索，例如 `name contains "access" and value = 5`。单击结果预览声明，双击或 Enter 固定文档；Ctrl+Enter 可打开副本，便于并排比较。

每个文档可独立选择 x86、x64 或 ARM64 架构，查看带语法高亮的声明、布局和虚表注释。Ctrl+单击类型引用可跳转，Alt+左右键返回或前进，Ctrl+F 在正文中查找。

界面支持中英文、DPI 缩放和高对比度；Windows 10 22H2 及更新系统还可选择浅色、深色或跟随系统外观。详细操作见 [GUI 使用指南](docs/gui.md)。

## 许可证

Winlex 采用 [GNU GPL v3.0](LICENSE)（`GPL-3.0-only`）许可证。第三方组件保留各自的许可证。

## 开发

使用 Rust MSVC 工具链。准备固定版本输入并运行核心测试：

```powershell
python scripts/prepare-inputs.py
cargo test -p winlex-core -p winlex-format -p winlex-cli --locked
```

构建步骤见 [构建与验证](docs/build.md)。

可选的 [UCRT 元数据生成工具](tools/ucrt-winmd/README.md)可从本机 Microsoft 头文件生成 x86/x64/ARM64 合并 WinMD。它独立构建，不是 Winlex 的构建依赖。

## 目录

- `src/`：程序入口。
- `crates/winlex-core/`：元数据读取、搜索、类型查询、ABI 计算和错误码查询。
- `crates/winlex-format/`：CLI 与 GUI 共用的声明展示和 JSON 输出适配。
- `crates/winlex-cli/`：命令行适配与集成测试。
- `crates/winlex-gui/`：原生 MDI 图形界面。
- `vendor/`：元数据读取器及本地补丁说明。
- `inputs/`：外部输入的版本、下载来源和校验值。
- `scripts/`：依赖准备与构建。
- `tools/ucrt-winmd/`：独立的 UCRT 查询元数据生成器。
- `tests/`：按 GUI、元数据、ABI 组织的独立验证套件。
