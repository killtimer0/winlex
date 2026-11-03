# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：GUI pytest 测试；`run.py` 运行测试，`resources.py` 检查资源，`remote.py` 调度本机或 SSH 目标。
- `metadata/`：`run.py` 运行读取器、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 C# fixture，`MetadataOracle/` 提供独立 .NET 读取器。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。
- `fixtures/build_message_fixtures.py`：使用双架构构建的 `.build.json` 生成消息资源 DLL，输出到 `tmp/message-fixtures/`；命令见构建指南。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

## GUI 测试职责

| 层次 | 职责 | 执行方式 |
| --- | --- | --- |
| Rust 单元／集成测试 | 解析、查询、模型、格式化，以及原生控件组件契约 | Cargo；纯逻辑不创建窗口，原生控件测试创建 HWND；应用级 pytest 不重复枚举组件算法 |
| CLI 进程契约 | JSON／文本输出、退出码、系统消息资源及路径 | `gui/test_error_code_cli.py` 复用二进制与资源 fixture；随默认隔离组执行 |
| GUI 窗口契约 | 命令与消息路由、控件状态、布局、实例／历史、生命周期，以及主动绘制结果 | 未标记 `interactive` 的 pytest 用例；默认独立桌面 |
| GUI 交互与呈现 | 真实点击命中、键盘／IME、拖拽、剪贴板、首帧及当前屏幕像素 | `@pytest.mark.interactive`；专用 VM 的独占活动会话 |

默认隔离组选择 CLI 与窗口契约；`--desktop current --real-input` 只选择交互与呈现用例，两组互斥。`--collect-only -q` 可查看所选组，再用 `-k` 缩小范围。未选中的用例显示为 deselected，不计为通过或跳过。

共享的 `preview_row()` 使用定向 Space 消息建立预览，验证预览状态而不承担鼠标命中契约；鼠标专项显式使用 `single_click_row()`／`double_click_row()`。增加用例时根据实际依赖标记，不通过全局运行选项改变同一用例的输入路径。

普通绘制测试可以主动重绘／`PrintWindow`。首帧、脏区域和残留像素必须读取当前画面，不能用主动重绘替代。独立桌面不隔离剪贴板等会话共享资源；交互用例仍需独占会话，测试互斥锁不能阻止人工操作。

## GUI 测试输入

将 `winlex.exe` 与被测 `winlex-gui.exe` 放在同一目录，以运行 CLI 工具测试。
自定义消息资源测试还需将 `tmp/message-fixtures/` 中生成的 DLL 部署到该目录，包括 `messages-*.dll`、`neutral-*.dll` 和 `empty-*.dll`。
缺少可选资源时，相应用例会跳过。UNC 用例使用测试机自身的管理共享，无访问权限时跳过。
