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

将 `Windows.Win32.winmd` 放在 `winlex.exe` 旁，或用 `--winmd PATH` 指定输入：

```powershell
winlex search OPENFILE --prefix
winlex value 0x80000000
winlex show CreateFileW --arch x64
winlex layout OVERLAPPED --arch x64
winlex vtable IStream --arch x64 --json
```

查询架构与程序自身架构无关。命令、输出及限制见 [CLI 文档](docs/cli.md)。

### 图形界面

`winlex-gui.exe` 提供 MDI 文档、文档标签栏、可拖动分栏和窗口平铺；实体文档固定架构，通过“以另一架构打开”并排比较同一快照。
点击文档标签切换活动文档，鼠标中键点击标签关闭对应文档；平铺时仍同时显示多个文档。
支持后台加载多个 WinMD、名称防抖搜索、分批追加结果和源限定的结构化声明展示。
文档只显示内容及相关操作，另一架构已有对应文档时直接激活复用。

```powershell
winlex-gui
winlex-gui --winmd Windows.Win32.winmd --winmd Windows.Wdk.winmd
```

DPI 默认自动选择；测试可通过 `WINLEX_DPI=pmv2|pmv1|system|unaware` 显式覆盖，不支持的模式会报错。
界面语言通过“设置 > 语言”保存，下次启动生效；默认跟随系统，不支持的系统语言回退英文。
`F6` 切换公共区域与文档，`Ctrl+L` 聚焦输入框；从搜索结果或来源列表打开文档。
模块职责和资源约定见 [GUI 设计](docs/gui.md)。

GUI 无显式输入时恢复保存的自动加载集合，首次启动为空；`--winmd` 仅覆盖本次输入，不自动修改保存集合。
GUI 不读取 `WINLEX_WINMD`，也不自动加载程序旁文件。使用左侧元数据面板的“添加”按钮或拖入文件发现来源；
左侧元数据面板单击条目打开或复用来源详情；详情显示路径、程序集版本、SHA256 和加载状态。
输入名称后自动搜索，Enter 可立即执行；结果分批追加，双击或 Enter 打开，F1 查看完整条目信息。
同一内容按 SHA256 去重，不同来源的同名声明分别显示。加载失败保留原目录；
来源详情页提供加载、卸载、重载和替换操作。卸载后旧文档仍使用打开时的不可变快照；文件变化后可显式打开当前版本。
用户主动加载/卸载会更新下次自动加载意图；配置保存在 `HKCU\Software\Winlex`，包括窗口正常位置、最大化状态和分栏宽度。

## 开发

使用 Rust MSVC 工具链。准备固定版本输入并运行核心测试：

```powershell
python scripts/prepare-inputs.py
cargo test -p winlex-core -p winlex-cli --release --locked
```

构建步骤见 [构建与验证](docs/build.md)。

## 目录

- `src/`：程序入口。
- `crates/winlex-core/`：同步查询库。
- `crates/winlex-cli/`：命令行适配与集成测试。
- `crates/winlex-gui/`：原生 MDI 图形界面。
- `vendor/`：元数据读取器及本地补丁说明。
- `inputs/`：外部输入的版本、下载来源和校验值。
- `scripts/`、`verify/`：依赖准备、构建和独立验证。
