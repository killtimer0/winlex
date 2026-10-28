# Winlex

离线 Windows API 参考查询工具，直接读取 WinMD 文件。

- 按名称、整数值和 GUID 搜索声明。
- 查看 API 参数、类型、回调及反向类型引用。
- 按 x86、x64、ARM64 ABI 计算结构布局、位域位置和 COM vtable 槽位。
- 支持文本和 JSON 输出。
- 可同时加载 SDK、WDK、WinRT 和 Windows App SDK 元数据，解析跨文件引用。
- 支持 WinRT/App SDK 声明、泛型、属性和事件查询，以及非泛型接口/委托槽位。

支持 Windows x86/x64，最低系统版本见[构建与验证](docs/build.md)。

## 使用

用 `--winmd PATH` 显式指定输入：

```powershell
winlex --winmd Windows.Win32.winmd search OPENFILE --prefix
winlex --winmd Windows.Win32.winmd value 0x80000000
winlex --winmd Windows.Win32.winmd show CreateFileW --arch x64
winlex --winmd Windows.Win32.winmd layout OVERLAPPED --arch x64
winlex --winmd Windows.Win32.winmd vtable IStream --arch x64 --json
```

查询架构与程序自身架构无关。命令、输出及限制见 [CLI 文档](docs/cli.md)。

### 图形界面

`winlex-gui.exe` 提供原生 MDI 多文档界面，支持后台加载多个 WinMD、名称搜索、来源管理和不同架构的声明比较。

```powershell
winlex-gui
winlex-gui --winmd Windows.Win32.winmd --winmd Windows.Wdk.winmd
```

无显式输入时恢复保存的自动加载集合，首次启动为空。通过“添加”或拖放发现来源，再从来源详情加载。
输入名称后自动搜索，Enter 立即执行，双击结果或按 Enter 打开文档；`Ctrl+L` 聚焦搜索框，`F6` 切换区域。
文档内可选择 x86、x64 或 ARM64 架构，原地更新声明；新文档优先使用上次显式选择的架构，无偏好时按 x86、x64、ARM64 顺序。“详细信息”显示完整路径和实体标识。

界面支持中英文和 DPI 缩放，语言在“设置 > 语言”中选择，下次启动生效。
来源、文档快照和配置约定见 [GUI 设计](docs/gui.md)。

## 开发

使用 Rust MSVC 工具链。准备固定版本输入并运行核心测试：

```powershell
python scripts/prepare-inputs.py
cargo test -p winlex-core -p winlex-cli --release --locked
```

构建步骤见 [构建与验证](docs/build.md)。

## 目录

- `src/`：程序入口。
- `crates/winlex-core/`：不可变元数据快照、类型化查询和搜索迭代器；普通结果支持 serde 序列化。
- `crates/winlex-format/`：CLI 与 GUI 共用的声明展示和 JSON 输出适配。
- `crates/winlex-cli/`：命令行适配与集成测试。
- `crates/winlex-gui/`：原生 MDI 图形界面。
- `vendor/`：元数据读取器及本地补丁说明。
- `inputs/`：外部输入的版本、下载来源和校验值。
- `scripts/`：依赖准备与构建。
- `tests/`：按 GUI、元数据、ABI 组织的独立验证套件。
