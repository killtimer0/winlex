# AGENTS.md

## 项目
Winlex：Windows 元数据查询工具，提供 CLI 和原生 Win32 MDI GUI。Rust。
- x86 必须支持 Windows XP RTM；x64 最低支持 XP x64 / Server 2003。原生界面固定使用宽字符。
- 旧系统兼容由 VC-LTL + YY-Thunks 提供；新增系统 API 前确认对应回退可用。

## 构建
- 使用 Rust MSVC 工具链和 Hostx64 的 Microsoft `link.exe`，链接器与目标架构匹配。
- 发布构建使用 `scripts/build.ps1`，覆盖 x86/x64；产物位于 `target/<Rust target>/release/`。运行 Clippy 检查。
- 构建前结束待替换的本项目实例，避免链接失败。

## 验证
- 检查构建日志中的 warning/error；不要按源文件名过滤，以免隐藏警告。

## 约定
- 自有文本遵循 `.gitattributes` 使用 LF；新增、编辑后检查换行，保留原编码和 BOM。
- 遵循现有风格，不加多余注释，不在注释中回应提示词。
- 用户文档只描述当前功能和使用方式，不写纠偏表述、内部实现或易变细节。
- 修改前检查已有模块、工具函数及 Windows 绑定，按既有职责复用；否则直接使用 Win32。不堆叠重复消息处理逻辑，不为单次使用建立通用框架。
- 固定 UI 文案放入 `.rc`／`STRINGTABLE`，使用资源 ID；协议值、日志和最小故障兜底除外。
- 固定选项集中定义显示资源 ID 与模型值；资源化修改须验证控件文本解析和选项往返映射。
- 路径使用 `Path`／`PathBuf` 传递，避免有损转换；验证 UNC 和长路径。
- 程序性填充控件时，避免触发非预期的变更处理。
