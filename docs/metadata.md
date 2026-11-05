# 获取元数据

从以下来源获取 `.winmd` 文件，提取到本地后即可在 Winlex 中打开。

## 选择来源

| 来源 | 用途 | 获取入口 |
| --- | --- | --- |
| Win32 | Windows 原生 API、结构与常量 | [Win32Metadata](https://www.nuget.org/packages/Microsoft.Windows.SDK.Win32Metadata) |
| WDK | 驱动开发 API 与结构 | [WdkMetadata](https://www.nuget.org/packages/Microsoft.Windows.WDK.Win32Metadata) |
| WinRT | Windows Runtime 类型和系统 API | [SDK Contracts](https://www.nuget.org/packages/Microsoft.Windows.SDK.Contracts)，或已安装 Windows SDK 的 `UnionMetadata` |
| UCRT | C 运行库声明与 x86／x64／ARM64 布局 | 由 Windows SDK 和 MSVC 头文件生成，暂未提供公开获取入口 |
| Windows App SDK／WinUI 3 | WinUI 控件及 Windows App SDK 类型 | [WinUI](https://www.nuget.org/packages/Microsoft.WindowsAppSDK.WinUI)，另需配套 Base、Foundation、InteractiveExperiences 元数据与 WinRT |
| Win2D | Windows Runtime 的二维绘图类型 | [Win2D](https://www.nuget.org/packages/Microsoft.Graphics.Win2D)，需配套 WinUI 与 WinRT |

选择版本时查看提供方的依赖与许可说明；提供校验清单时，按对应版本核对下载或生成的文件。

## 添加到 Winlex

1. 将包中的 `.winmd` 提取到本地目录，保留其依赖与来源信息。
2. 在 GUI 中选择“打开文件”或将文件拖入窗口；CLI 则在查询命令前列出文件路径。
3. 查询跨来源类型时，将所需依赖文件一起添加。相同组件的不同版本／架构通常应选择其一。

```powershell
winlex.exe .\Windows.Win32.winmd search CreateFileW
winlex.exe .\UCRT.winmd layout _stat64 --arch x64
Get-FileHash .\Windows.Win32.winmd -Algorithm SHA256
```

WinRT Contracts 与 SDK 合并版 `Windows.winmd` 选择一套加载，避免重复定义。

## 许可与更新

使用或再分发元数据及头文件派生成品时，请遵循提供方的许可条款。更新时核对来源、版本、SHA-256 和配套依赖。
