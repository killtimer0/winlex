# 获取元数据

Winlex 离线读取本地 `.winmd` 文件，不负责下载、解压或自动更新。元数据不随 Winlex 的程序包分发，也不要求与 Winlex 版本一致。

## 选择来源

| 来源 | 用途 | 固定版本与官方获取入口 |
| --- | --- | --- |
| Win32 | Windows 原生 API、结构与常量 | [Win32Metadata 71.0.37-preview](https://www.nuget.org/packages/Microsoft.Windows.SDK.Win32Metadata/71.0.37-preview) |
| WDK | 驱动开发 API 与结构 | [WdkMetadata 0.13.25-experimental](https://www.nuget.org/packages/Microsoft.Windows.WDK.Win32Metadata/0.13.25-experimental)，上游尚无稳定版 |
| WinRT | Windows Runtime 类型和系统 API | [SDK Contracts 10.0.28000.2957](https://www.nuget.org/packages/Microsoft.Windows.SDK.Contracts/10.0.28000.2957)，或已安装 Windows SDK 的 `UnionMetadata` |
| UCRT | C 运行库声明与 x86／x64／ARM64 布局 | `winmd-collection` 中的 `tools/ucrt-winmd`，从 SDK 10.0.26100.0／MSVC 14.51.36231 头文件生成 |
| Windows App SDK／WinUI 3 | WinUI 控件及 Windows App SDK 类型 | [WinUI 2.6.5](https://www.nuget.org/packages/Microsoft.WindowsAppSDK.WinUI/2.6.5)，另需配套 Base、Foundation、InteractiveExperiences 元数据与 WinRT |
| Win2D | Windows Runtime 的二维绘图类型 | [Win2D 1.4.0](https://www.nuget.org/packages/Microsoft.Graphics.Win2D/1.4.0)，需配套 WinUI 与 WinRT |

独立的 `winmd-collection` 项目提供固定来源、版本、SHA-256 及提取和生成脚本；其公开仓库和成品托管地址待确定，当前不提供未经确认的成品下载链接。也可直接从上述官方来源获取。

### 校验示例

以下是上述版本提取后代表性 WinMD 的 SHA-256，不是 NuGet 包或 ZIP 的哈希。Contracts 共含 96 个 WinMD；WinUI 包含多份配套元数据。完整成员清单、包哈希及许可位置由 `winmd-collection/sources/*.json` 记录，准备成品后还提供逐文件 `SHA256SUMS`。

```text
73e57def24d91f2ec025238100099d5333dd441cab5e843cd2fa8f0617472d56  Windows.Win32.winmd
54b8e48866b0a7e066981b63bb6f82fc6947fe8c57e2b5cb4513b1537aa836c2  Windows.Wdk.winmd
3775a216e50fa417f5dc262a776a0ee32e12837bd22e8450339aa8193e3db5c1  Windows.Foundation.UniversalApiContract.winmd
2eb555925ed830c0665f7a7cbcb8a9807c57ab75434b21cea8d1cae3dfbd05e3  UCRT.winmd
72bcc3558747aa127364e7b06e55dd165784a15a77efc640b0c18080579d167f  Microsoft.UI.Xaml.winmd
66305eea6238f467e0a311a229d267e0d139d0d6384534203a2d4ee43dad6e6f  Microsoft.Graphics.Canvas.winmd
```

UCRT 哈希对应记录中的生成器和头文件版本；更换输入后应使用重新生成的 manifest，而不是沿用旧哈希。Win2D 包内两份 WinMD 字节相同，只选择一份。

## 添加到 Winlex

1. 将包中的 `.winmd` 提取到本地目录，保留其依赖与来源信息。
2. 在 GUI 的来源面板选择“添加”，或将文件拖入窗口；CLI 则在查询命令前列出文件路径。
3. 查询跨来源类型时，将所需依赖文件一起添加。相同组件的不同版本／架构通常应选择其一，不要把所有包内文件无差别混装。

```powershell
winlex.exe .\Windows.Win32.winmd search CreateFileW
winlex.exe .\UCRT.winmd layout _stat64 --arch x64
Get-FileHash .\Windows.Win32.winmd -Algorithm SHA256
```

WinRT Contracts 是 SDK 单体 `Windows.winmd` 的替代组织形式，通常不与后者同时加载。UCRT 元数据仅描述声明与布局，不提供运行库实现。组件本身所需的 Windows 版本不等于 Winlex 读取这些文件的最低系统版本。

## 许可与更新

官方元数据及头文件派生成品适用各自条款，工具源码的开源许可不覆盖输入文件。公开再分发授权未确认的成品不在此提供镜像。更新时重新校验来源、版本、SHA-256 和配套依赖；Winlex 不在后台检查或对齐版本。
