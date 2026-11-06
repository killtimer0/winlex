# 独立验证

从仓库根目录运行；依赖与命令见 [构建与验证](../docs/build.md)。

- `gui/`：GUI pytest 测试；`run.py` 在目标端运行测试，`resources.py` 在开发机源码树中检查资源，`remote.py` 调度本机或 SSH 目标。
- `metadata/`：`run.py` 运行 CLI、fixture、跨源及可选 WinRT 检查；`ReaderFixtures/` 保存 CLI 行为用 C# fixture。
- `differential/`：WinMD 与 .NET 读取结果的独立对拍及回归测试。
- `abi/`：`run.py` 使用 SDK 编译器验证布局、位域和 vtable；也可由 `metadata/run.py --abi` 调用。
- `fixtures/build_message_fixtures.py`：使用双架构构建的 `.build.json` 生成消息资源 DLL，输出到 `tmp/message-fixtures/`；命令见构建指南。

报告与临时 fixture 写入 `tmp/`。Rust 单元测试位于源码中，集成测试位于各 crate 的 `tests/`，验证用 Rust 可执行程序位于 Cargo `examples/`。

## GUI 测试

开发机使用根目录 uv 项目的 `gui` 依赖组；XP 目标机使用兼容 XP 的 Python 3.8 和 `gui/requirements.lock`，不共用开发机 `.venv`。安装和运行命令见[构建与验证](../docs/build.md)。

目标配置放在 `gui/config/<target>.json`；新增目标从 `gui/config/target.example.json` 复制，字段说明和调度命令见[本机/SSH 调度](../docs/build.md#本机ssh-调度)。

默认在独立的非活动桌面运行 CLI 和窗口契约测试。`--desktop current --real-input` 只运行标记为 `interactive` 的交互与屏幕呈现测试，两组互斥。使用 `--collect-only -q` 查看所选用例，或用 `-k` 缩小范围。

普通契约测试不需要前台或独占键鼠，但独立桌面不隔离剪贴板等会话共享资源。交互测试需要已解锁的独占活动桌面，运行期间不要操作键鼠；建议使用专用 VM。远程提交默认使用独立桌面，交互测试有相同要求。

维护屏幕呈现测试时，首帧和残留像素检查必须读取当前画面，不能用主动重绘代替；验证键鼠交互时应使用真实输入。

## GUI 测试输入

将 `winlex.exe` 与被测 `winlex-gui.exe` 放在同一目录，以运行 CLI 工具测试。
自定义消息资源测试还需将 `tmp/message-fixtures/` 中生成的 DLL 部署到该目录，包括 `messages-*.dll`、`neutral-*.dll` 和 `empty-*.dll`。
缺少可选资源时，相应用例会跳过。UNC 用例使用测试机自身的管理共享，无访问权限时跳过。
