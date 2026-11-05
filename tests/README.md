# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：GUI pytest 测试；`run.py` 运行测试，`resources.py` 检查资源，`remote.py` 调度本机或 SSH 目标。
- `metadata/`：`run.py` 运行 CLI、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 CLI 行为用 C# fixture。
- `differential/`：WinMD 与 .NET 读取结果的独立对拍及回归测试。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。
- `fixtures/build_message_fixtures.py`：使用双架构构建的 `.build.json` 生成消息资源 DLL，输出到 `tmp/message-fixtures/`；命令见构建指南。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

## GUI 测试

默认隔离桌面运行 CLI 和窗口契约测试；`--desktop current --real-input` 只运行标记为 `interactive` 的交互与屏幕呈现测试，两组互斥。使用 `--collect-only -q` 查看所选用例，或用 `-k` 缩小范围。

交互测试需要专用 VM 中解锁的独占活动会话；测试期间不要操作键鼠。独立桌面不隔离剪贴板等会话共享资源。

首帧和残留像素测试应读取当前画面，不能用主动重绘代替；键鼠交互测试应使用真实输入。

## GUI 测试输入

将 `winlex.exe` 与被测 `winlex-gui.exe` 放在同一目录，以运行 CLI 工具测试。
自定义消息资源测试还需将 `tmp/message-fixtures/` 中生成的 DLL 部署到该目录，包括 `messages-*.dll`、`neutral-*.dll` 和 `empty-*.dll`。
缺少可选资源时，相应用例会跳过。UNC 用例使用测试机自身的管理共享，无访问权限时跳过。
