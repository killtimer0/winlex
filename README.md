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
- `vendor/`：元数据读取器及本地补丁说明。
- `inputs/`：外部输入的版本、下载来源和校验值。
- `scripts/`、`verify/`：依赖准备、构建和独立验证。
