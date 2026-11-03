# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：GUI pytest 测试；`run.py` 运行测试，`resources.py` 检查资源，`remote.py` 调度本机或 SSH 目标。
- `metadata/`：`run.py` 运行读取器、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 C# fixture，`MetadataOracle/` 提供独立 .NET 读取器。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。
- `fixtures/build_message_fixtures.py`：使用双架构构建的 `.build.json` 生成消息资源 DLL，输出到 `tmp/message-fixtures/`；命令见构建指南。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

## GUI 测试输入

将 `winlex.exe` 与被测 `winlex-gui.exe` 放在同一目录，以运行 CLI 工具测试。
自定义消息资源测试还需将 `tmp/message-fixtures/` 中生成的 DLL 部署到该目录，包括 `messages-*.dll`、`neutral-*.dll` 和 `empty-*.dll`。
缺少可选资源时，相应用例会跳过。UNC 用例使用测试机自身的管理共享，无访问权限时跳过。
