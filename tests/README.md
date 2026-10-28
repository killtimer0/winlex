# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：pytest 用例、`conftest.py` fixture、`support/` 原生辅助代码；`run.py` 管理隔离与清理，`resources.py` 检查资源，`remote.py` 管理远程队列。
- `metadata/`：`run.py` 运行读取器、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 C# fixture，`MetadataOracle/` 提供独立 .NET 读取器。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

GUI 用例使用 `win32con` 的常量；`win32con` 未提供的工具栏、列表视图、RichEdit 消息集中在 `gui/support/messages.py`，并按头文件注明来源。
