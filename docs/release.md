# 手动构建与交付

## 主程序

`.github/workflows/build.yml` 只接受手动触发（`workflow_dispatch`），不随普通提交或定时运行，也不自动创建 Release。

1. 将源码及工作流上传到用户自己的 GitHub 仓库。首次使用时，工作流须位于默认分支并启用 Actions。
2. 打开 Actions → **Build distribution artifacts** → **Run workflow**，选择分支并确认。
3. 运行结束后，在该次运行的 Artifacts 中分别获取 `winlex-x86`、`winlex-x64`、`winlex-arm64`。
4. 每个 artifact 含对应 ZIP 与 `.zip.sha256`；ZIP 内包含 CLI、GUI、必要许可证和第三方声明，以及逐文件 `SHA256SUMS.txt`。

流水线使用 `Dist` profile，且不生成、下载或捆绑 WinMD。所有架构使用固定版本 VC-LTL；x86／x64 配合 YY-Thunks，ARM64 使用系统 UCRT 模式且不链接 YY-Thunks。最低系统和构建环境见[构建指南](build.md)。交叉构建不等同于目标系统实际运行验证。

手动工作流先统一检查格式并运行宿主库测试，再分别执行三架构 Debug 构建及构建／打包契约，最后生成 Dist 包。各架构使用相同入口，只有实际工具和兼容库需求不同。Rust 使用最新 stable，不固定版本；构建记录保留实际版本。

本地使用 PowerShell 7.2+ 构建，再将构建输出显示的 `.build.json` 路径显式传给打包脚本：

```powershell
./scripts/build.ps1 -Architecture x64 -Configuration Dist
./scripts/package.ps1 -Architecture x64 `
  -CliBuildRecord "$cli.build.json" -GuiBuildRecord "$gui.build.json" `
  -OutputDir ./tmp/packages
Get-FileHash ./tmp/packages/winlex-x64.zip -Algorithm SHA256
```

`$cli`、`$gui` 为构建显示的可执行文件绝对路径。两份记录的目标策略和配置必须一致，可以来自不同次构建。记录中的源码与工具版本用于追溯，不要求与打包时的工作区或工具链相同；打包时应确认选择了预期的 CLI 和 GUI 版本。构建记录使用 schema 2；旧记录需重新构建生成。

打包脚本核对构建记录与二进制哈希，并按记录中的策略检查 PE 架构／子系统及普通和延迟 CRT 导入；归档后验证 ZIP 内文件集合与校验值，拒绝额外运行库、WinMD 和构建记录进入包。许可材料从当前源码、Cargo 依赖及 Rust 安装收集，应使用与待分发产物对应的材料。输出可放在 `tmp/` 或 `artifacts/`；已有 ZIP 或校验文件不会被覆盖。默认只接受 Dist 记录，开发验证可显式指定 `-Configuration Debug`。二进制构建记录含本机路径，不放入程序包。

这些措施提供输入追溯和产物校验，不保证 EXE 或 ZIP 字节级可复现；MSVC、SDK、Rust 版本以及归档时间戳仍可能变化。

## 公开分发前

- 核对主程序第三方声明、VC-LTL 对象级授权和 GPL 组合分发条件；现有未确认项不会因成功构建或打包而自动解决。
- 保留程序依赖的许可、版权、来源版本和校验信息。
- 验证目标系统上的程序行为，尤其是未经真机验证的架构与最低系统。远程流水线运行结果须由实际触发后的日志确认。
