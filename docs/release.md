# 构建与交付

## 主程序

通过 `.github/workflows/build.yml` 构建分发包：

1. 将源码及工作流上传到用户自己的 GitHub 仓库。首次使用时，工作流须位于默认分支并启用 Actions。
2. 打开 Actions → **Build distribution artifacts** → **Run workflow**，选择分支并确认。
3. 运行结束后，在该次运行的 Artifacts 中获取 `winlex-delivery`。带 `intermediate` 的 artifact 是按架构汇总前的中间材料，不是完整交付集合。
4. 最终集合包含 `winlex-x86.zip`、`winlex-x64.zip`、`winlex-arm64.zip`、公共 `licenses.zip` 和统一的 `SHA256SUMS`。

程序 ZIP 包含 CLI、GUI、GPL 许可证、第三方声明、许可材料获取说明及包内 `SHA256SUMS.txt`。公共许可证包包含三架构详细许可材料的并集和不含本机路径的构建来源记录。下载对应架构的程序包和同版本 `licenses.zip`，将两者解压到同一目录，使声明中的相对链接可用；再分发须同时提供匹配材料并履行适用的源码提供义务。

可用 `sha256sum --check SHA256SUMS` 检查下载的全部 ZIP；Windows 可通过 `Get-FileHash -Algorithm SHA256` 与清单逐项比对。元数据须另外下载，程序包不包含 WinMD。GitHub 自动生成的 Source code 压缩包不是可执行程序包。

流水线使用 `Dist` profile。最低系统、兼容库和构建环境见[构建指南](build.md)。

工作流中的检查和工具链配置见 `.github/workflows/build.yml`。

### tag 与草稿 Release

推送 `v*` tag 会运行同一条检查、三架构构建和汇总流程；仅 tag 推送成功后创建草稿 Release，含连字符的 tag 标记为 prerelease。手动运行只生成 artifacts，即使选择 tag 也不创建 Release。普通分支 push 和 PR 不触发此工作流。

发布任务只下载并校验已验证的交付 artifact，不重新构建。已有同名 Release 时失败，不自动覆盖附件。草稿不自动公开，须完成下方审核后人工发布。

### 本地打包

本地使用 PowerShell 7.2+ 构建，安装 Rust 的 `rust-docs` 组件以收集许可材料，再将构建输出显示的 `.build.json` 路径传给打包脚本：

```powershell
rustup component add rust-docs
./scripts/build.ps1 -Architecture x64 -Configuration Dist
./scripts/package.ps1 -Architecture x64 `
  -CliBuildRecord "$cli.build.json" -GuiBuildRecord "$gui.build.json" `
  -OutputDir ./tmp/packages
Get-FileHash ./tmp/packages/winlex-x64.zip -Algorithm SHA256
```

每次打包还生成 `winlex-ARCH-licenses.zip` 中间许可证包和程序 ZIP 的 `.sha256`。对三个架构分别执行构建和打包后，汇总到一个尚不存在的输出目录：

```powershell
uv run --locked python scripts/delivery.py assemble --inputs ./tmp/packages --output ./artifacts/delivery
```

汇总默认要求 Dist，验证三架构 PE／CRT、程序和许可校验值、匹配的构建来源，并对同路径许可内容冲突报错。CI 还要求构建来自本次提交且源码干净。Debug 打包和汇总均须显式指定配置：PowerShell 使用 `-Configuration Debug`，Python 使用 `--configuration Debug`。

`$cli`、`$gui` 为构建显示的可执行文件绝对路径。两份记录的目标策略和配置必须一致，可以来自不同次构建；打包前应确认选择了预期的 CLI 和 GUI 版本。不受支持的旧格式记录需重新构建生成。

打包脚本核对构建记录、二进制哈希、目标架构和 CRT 导入，并验证归档校验值；许可材料从当前源码、Cargo 依赖及 Rust 安装收集，应使用与待分发产物对应的材料。输出可放在 `tmp/` 或 `artifacts/`；已有输出不会被覆盖。构建记录含本机路径，不放入交付包；公共许可证包仅保留来源提交、工具版本、目标策略和输入／二进制哈希等可移植信息。

## 公开分发前

- 核对主程序第三方声明；VC-LTL 对象级授权和 GPL 组合分发条件仍待确认。
- 保留程序依赖的许可、版权、来源版本和校验信息。
- 检查打包警告，核对上游未附独立许可正文的依赖材料。
- 核对 GPL 等许可证的完整对应源码提供方式，不能仅以自动生成的 Source code 附件认定已履行义务。
- 验证目标系统上的程序行为，尤其是未经真机验证的架构与最低系统。
