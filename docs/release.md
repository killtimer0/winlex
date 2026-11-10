# 构建与交付

## 主程序

通过 `.github/workflows/build.yml` 构建分发包：

1. 将源码及工作流上传到自己的 GitHub 仓库，在默认分支中添加工作流并启用 Actions。
2. 打开 Actions → **Build distribution artifacts** → **Run workflow**，选择分支并确认。
3. 运行结束后，在该次运行的 Artifacts 中获取完整交付包 `winlex-delivery`。带 `intermediate` 的 artifact 保存各架构汇总前的中间材料。
4. 最终集合包含三个架构的 `winlex-ARCH.zip` 程序包、`winlex-ARCH-symbols.zip` 符号包、公共 `licenses.zip` 和统一的 `SHA256SUMS`，共八个文件。

下载对应架构的程序包和同版本 `licenses.zip`，解压到同一目录，即可通过第三方声明中的相对链接查看许可材料。

调试或分析崩溃转储时，下载与程序同版本、同架构的符号包，其中包含 `winlex.pdb`、`winlex_gui.pdb` 和包内校验清单。符号来自程序的同次构建，打包时按 EXE/PDB 的 GUID 和 Age 校验匹配。PDB 包含调试信息，也可能包含构建机路径。

可用 `sha256sum --check SHA256SUMS` 检查下载的全部 ZIP；Windows 可通过 `Get-FileHash -Algorithm SHA256` 与清单逐项比对。WinMD 元数据从 [winmd-collection Releases](https://github.com/killtimer0/winmd-collection/releases) 另行下载。

流水线使用 `Dist` profile，检查与工具配置见[工作流](../.github/workflows/build.yml)；最低系统、兼容库和本地环境见[构建指南](build.md)。

### tag 与草稿 Release

推送 `v*` tag 会运行同一条检查、三架构构建和汇总流程；仅 tag 推送成功后创建草稿 Release，含连字符的 tag 标记为 prerelease。手动运行只生成 artifacts，即使选择 tag 也不创建 Release。普通分支 push 和 PR 不触发此工作流。

发布版本只在 `Cargo.toml` 的 `[workspace.package] version` 修改，并运行 `cargo update --workspace` 更新 `Cargo.lock`；tag 必须为 `v<version>`，预发布可追加 `-<suffix>`。CLI 版本输出、GUI 文件版本和嵌入的 manifest 都由该值生成。

发布任务复用交付 artifact，并在上传前校验文件。已有同名 Release 时失败。生成的草稿可在 GitHub Releases 页面手动发布。

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

每次打包还生成 `winlex-ARCH-symbols.zip`、`winlex-ARCH-licenses.zip` 中间许可证包和程序 ZIP 的 `.sha256`。对三个架构分别执行构建和打包后，汇总到一个尚不存在的输出目录：

```powershell
uv run --locked python scripts/delivery.py assemble --inputs ./tmp/packages --output ./artifacts/delivery
```

汇总默认要求 Dist，验证三架构 PE／CRT、程序和许可校验值、匹配的构建来源，并对同路径许可内容冲突报错。CI 还要求构建来自本次提交且源码干净。Debug 打包和汇总均须显式指定配置：PowerShell 使用 `-Configuration Debug`，Python 使用 `--configuration Debug`。

`$cli`、`$gui` 为构建显示的可执行文件绝对路径。打包支持目标策略和配置一致的两份记录，也支持分别构建的 CLI 和 GUI。旧格式记录可通过重新构建更新。

打包脚本从当前源码、Cargo 依赖及 Rust 安装收集许可材料。程序包内含 `SHA256SUMS.txt`；公共许可证包合并三架构许可和可移植构建来源。输出目录中的已有文件会触发重名错误。
