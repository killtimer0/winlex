# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：pytest 用例、`conftest.py` fixture、`support/` 原生辅助代码；`run.py` 管理隔离与清理，`resources.py` 检查资源，`remote.py` 管理远程队列。
- `metadata/`：`run.py` 运行读取器、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 C# fixture，`MetadataOracle/` 提供独立 .NET 读取器。
- 默认 Debug，使用 Cargo 报告的实际可执行文件路径，遵循 target／target-dir 配置。`metadata/run.py --target-dir "PATH WITH SPACES"` 可指定输出目录，`--profile release` 可显式选择优化版本。
- `metadata/run.py` 将产物路径写入 `tmp/verify/artifacts.json`（可用 `--artifacts PATH` 更改）。各 gate 可独立运行，通过 `WINLEX_GATE_ARTIFACTS` 选择记录；`WINLEX_GATE_CLI`、`WINLEX_GATE_READER_FACTS`、`WINLEX_GATE_QUERY_AUDIT` 可分别覆盖路径。
- `python -m unittest discover -s tests/metadata -p test_artifacts.py` 运行不依赖 Cargo／SDK 的产物选择契约测试。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。
- `fixtures/build_message_fixtures.py --x86-build PATH --x64-build PATH`：显式传入双架构可执行文件的 `.build.json`，使用其中的 SDK 工具生成无入口的消息资源 DLL，输出到 `tmp/message-fixtures/`。包含双语消息、语言中立消息和无消息表文件，覆盖 ANSI／Unicode 及旧式和编号占位符。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

GUI 用例使用 `win32con` 的常量；`win32con` 未提供的工具栏、列表视图、RichEdit 消息集中在 `gui/support/messages.py`，并按头文件注明来源。

`gui/test_error_code_cli.py` 在 Windows 测试机运行 GUI 可执行文件同目录的 `winlex.exe`，验证无需元数据的工具命令。自定义资源、长路径和 UNC 用例需要将生成的 `messages-x86.dll` 与 `messages-x64.dll` 一同部署到该目录；缺少资源时明确跳过。UNC 用例使用测试机自身的管理共享，无访问权限时跳过。
