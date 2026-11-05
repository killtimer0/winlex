# 手动构建与交付

## 主程序

通过 `.github/workflows/build.yml` 手动构建分发包：

1. 将源码及工作流上传到用户自己的 GitHub 仓库。首次使用时，工作流须位于默认分支并启用 Actions。
2. 打开 Actions → **Build distribution artifacts** → **Run workflow**，选择分支并确认。
3. 运行结束后，在该次运行的 Artifacts 中分别获取 `winlex-x86`、`winlex-x64`、`winlex-arm64`。
4. 每个 artifact 含对应 ZIP 与 `.zip.sha256`；ZIP 内包含 CLI、GUI、必要许可证和第三方声明，以及逐文件 `SHA256SUMS.txt`。

流水线使用 `Dist` profile。最低系统、兼容库和构建环境见[构建指南](build.md)。

手动工作流先检查格式并运行宿主库测试，再对各架构执行 Debug 构建及构建／打包契约，最后生成 Dist 包。Rust 使用最新 stable，构建记录保留实际版本。

本地使用 PowerShell 7.2+ 构建，安装 Rust 的 `rust-docs` 组件以收集许可材料，再将构建输出显示的 `.build.json` 路径传给打包脚本：

```powershell
rustup component add rust-docs
./scripts/build.ps1 -Architecture x64 -Configuration Dist
./scripts/package.ps1 -Architecture x64 `
  -CliBuildRecord "$cli.build.json" -GuiBuildRecord "$gui.build.json" `
  -OutputDir ./tmp/packages
Get-FileHash ./tmp/packages/winlex-x64.zip -Algorithm SHA256
```

`$cli`、`$gui` 为构建显示的可执行文件绝对路径。两份记录的目标策略和配置必须一致，可以来自不同次构建；打包前应确认选择了预期的 CLI 和 GUI 版本。不受支持的旧格式记录需重新构建生成。

打包脚本核对构建记录与二进制哈希，并按记录中的策略检查 PE 架构／子系统及普通和延迟 CRT 导入；归档后验证 ZIP 内文件集合与校验值，拒绝额外运行库、WinMD 和构建记录进入包。许可材料从当前源码、Cargo 依赖及 Rust 安装收集，应使用与待分发产物对应的材料。输出可放在 `tmp/` 或 `artifacts/`；已有 ZIP 或校验文件不会被覆盖。默认只接受 Dist 记录，开发验证可显式指定 `-Configuration Debug`。二进制构建记录含本机路径，不放入程序包。

## 公开分发前

- 核对主程序第三方声明；VC-LTL 对象级授权和 GPL 组合分发条件仍待确认。
- 保留程序依赖的许可、版权、来源版本和校验信息。
- 验证目标系统上的程序行为，尤其是未经真机验证的架构与最低系统。
