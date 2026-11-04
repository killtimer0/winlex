# 手动构建与交付

## 主程序

`.github/workflows/build.yml` 只接受手动触发（`workflow_dispatch`），不随普通提交或定时运行，也不自动创建 Release。

1. 将源码及工作流上传到用户自己的 GitHub 仓库。首次使用时，工作流须位于默认分支并启用 Actions。
2. 打开 Actions → **Build distribution artifacts** → **Run workflow**，选择分支并确认。
3. 运行结束后，在该次运行的 Artifacts 中分别获取 `winlex-x86`、`winlex-x64`、`winlex-arm64`。
4. 每个 artifact 含对应 ZIP 与 `.zip.sha256`；ZIP 内包含 CLI、GUI、必要许可证和第三方声明，以及逐文件 `SHA256SUMS.txt`。

流水线使用 `Dist` profile，且不生成、下载或捆绑 WinMD。x86／x64 采用固定版本 VC-LTL／YY-Thunks；ARM64 使用静态 MSVC CRT。最低系统和构建环境见[构建指南](build.md)。交叉构建不等同于目标系统实际运行验证。

本地从 MSVC 开发环境使用 PowerShell 7.2+ 构建，再将构建输出显示的 `.build.json` 路径显式传给打包脚本：

```powershell
./scripts/build.ps1 -Architecture x64 -Configuration Dist -LinkExe $link `
  -YyThunksObject $thunks -VcLtlRoot $ltl
./scripts/package.ps1 -Architecture x64 `
  -CliBuildRecord "$cli.build.json" -GuiBuildRecord "$gui.build.json" `
  -OutputDir ./tmp/packages
Get-FileHash ./tmp/packages/winlex-x64.zip -Algorithm SHA256
```

`$cli`、`$gui` 为本次构建显示的可执行文件绝对路径。打包脚本核对目标、配置和二进制哈希，拒绝覆盖已有 ZIP 或校验文件；默认只接受 Dist 记录，开发验证可显式指定 `-Configuration Debug`。二进制构建记录含本机路径，不放入程序包。

## 元数据

`winmd-collection` 是独立本地仓库，不作为 Winlex 子模块。其脚本在本地准备六类元数据，按来源输出压缩包、版本和 SHA-256 清单；上传方法见该项目的 `UPLOAD.md`。

公开托管仓库与成品位置确定后，更新 Winlex 的 [`metadata.md`](metadata.md)。GUI 打开的是 Winlex 主仓库的此固定指南；指南中的官方获取入口不依赖新仓库地址。不要用占位 URL 代替尚不存在的下载链接。

## 公开分发前

- 核对主程序第三方声明、VC-LTL 对象级授权和 GPL 组合分发条件；现有未确认项不会因成功构建或打包而自动解决。
- 分别确认微软原始 WinMD、附属组件元数据及头文件派生 UCRT 成品的再分发条件；未确认前不公开上传这些成品。
- 保留相应许可、版权、来源版本和校验信息；UCRT 原始生成 manifest 含本机路径，分享前审查。
- 验证目标系统上的程序行为，尤其是未经真机验证的架构与最低系统。远程流水线运行结果须由实际触发后的日志确认。
