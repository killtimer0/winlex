# CLI 与查询接口

```text
winlex [选项] 命令 [查询]
```

输入查找顺序：`--winmd PATH`、`WINLEX_WINMD` 环境变量、程序旁的 `Windows.Win32.winmd`。
重复 `--winmd` 可加载多个文件；内容相同的文件按 SHA256 去重，来源按哈希排序。

```powershell
winlex --winmd Windows.Win32.winmd --winmd Windows.Wdk.winmd summary
winlex --winmd Windows.Win32.winmd --winmd Windows.Wdk.winmd show Windows.Wdk.winmd::NtCancelIoFileEx --arch x64
```

## 命令

| 命令 | 用途 |
|---|---|
| `summary` | 来源、程序集身份及表行数 |
| `search TEXT` | 名称子串搜索；支持 `--exact`、`--prefix` |
| `value INTEGER` | 十进制或 `0x` 十六进制整数搜索 |
| `guid GUID` | GUID 搜索，允许外层花括号 |
| `show NAME或ID` | 类型、成员或类型引用详情 |
| `layout TYPE --arch ARCH` | 大小、对齐、字段和位域偏移 |
| `vtable TYPE --arch ARCH` | COM、WinRT 接口及委托的方法槽位 |
| `references TYPE --arch ARCH` | 字段、方法、属性、事件签名及接口要求中的反向引用 |

`ARCH` 可选 `x86`、`x64`、`arm64`。搜索分页使用 `--limit`（默认50，上限1000）和 `--offset`。
`--help`、`--version` 单独调用。

搜索不区分大小写。声明选择优先匹配精确大小写，再尝试忽略大小写。
存在多个候选时，使用架构、限定名或 token 消除歧义。限定名形如 `Namespace.Type.Field`，嵌套类型形如 `Namespace.Outer+Inner`。
实体 ID 为 `SOURCE_SHA256::0xTOKEN`。选择声明时支持 `SOURCE::NAME` 和 `SOURCE::0xTOKEN`，SOURCE 可用 SHA256 或唯一程序集名称（包括名称中的 `.winmd` 后缀）。裸 token 仅允许单来源。
`--source SOURCE` 在搜索时过滤来源，在 show/layout/vtable/references 时限定目标来源；反向引用仍查找所有已加载来源。独立来源中的同名声明不会自动合并。
非 WinRT 引用按程序集名称、版本、culture、公钥 token 和 content type 匹配；依赖须显式加载。

## 输出

默认输出文本；`--json` 输出一份完整 JSON 至 stdout：

```json
{"schema":"winlex-cli-2","ok":true,"sources":[],"source_sha256":null,"command":"show","result":{}}
```

失败时：

```json
{"schema":"winlex-cli-2","ok":false,"error":{"code":"query_error","message":"..."}}
```

退出码：0成功、2参数错误、3输入或查询失败、4输出失败。文本模式错误写入 stderr。

- 整数以十进制字符串和保留位宽的十六进制表示；数值搜索按数学整数比较，`-1` 不等于 `0xffffffff`。
- `sources` 提供文件哈希、路径、程序集身份和依赖；`source_sha256` 仅在单来源时非 null。
- 类型签名保留具名引用的 `reference_id`，可用 `show ID --arch ARCH` 继续导航；解析后的 TypeRef 提供 `target_id`。
- 布局单位为 bit，偏移相对直接包含它的聚合；位域另提供相对底层字段的偏移。
- vtable 同时提供槽位和字节偏移。

## WinRT 与 Windows App SDK

可加载 Windows SDK 合并版 `Windows.winmd`，或通过重复 `--winmd` 加载分立 contracts。Windows App SDK 的 WinMD 位于各组件 NuGet 包的 `metadata/` 下；所需依赖也须显式加载。

- WinRT 引用按类型全名匹配已加载的 WinRT 定义。合并版与分立 contracts 应选择一套，重叠定义会导致歧义。
- 声明保留类型/方法泛型参数、实例化参数、属性、事件和 accessor 方法 ID。`interface_implementations` 提供默认接口标记及关系属性，`base_type` 提供基类。
- runtime class 的静态、激活和组合工厂信息保留在 `StaticAttribute`、`ActivatableAttribute`、`ComposableAttribute` 中。
- 非泛型 WinRT 接口的 vtable 包含 `IUnknown`、`IInspectable` 六个基础槽位；委托包含 `IUnknown` 三槽及 `Invoke`。隐式槽位的实体 ID 为 null。
- `show` 提供元数据签名；WinRT 方法 ABI 签名展开尚不支持。

## Rust API

`winlex_core::Catalog` 提供同步查询，可通过 `Arc` 跨线程共享，结果拥有独立数据。
搜索和反向引用支持 `SearchOptions.cancel` 协作式取消；单次描述和布局查询不提供取消点。

```rust,ignore
use winlex_core::{Architecture, Catalog, SearchOptions};

let catalog = Catalog::open("Windows.Win32.winmd")?;
let matches = catalog.search("OPENFILE", SearchOptions::default())?;
let layout = catalog.layout("OVERLAPPED", Architecture::X64)?;
```

分页结果使用 Rust 结构体，详细结果使用 `serde_json::Value`，错误使用 `anyhow::Result`。
多源使用 `Catalog::open_many(paths)`；`sources()` 枚举来源，`SearchOptions.source` 过滤搜索或反向引用结果的来源。

## 限制

- 开放泛型的布局/vtable、泛型实例 ABI 及部分 WinRT 字段类型（例如 HSTRING、System.Guid）尚不支持计算。
- 暂不支持 ModuleRef、类型转发和 retargetable 程序集引用。
- 非零显式偏移、显式大小和灵活数组等尚不支持布局计算。
- 反向引用不包含属性字符串、宏或文档关系。
- 不提供完整文档正文、函数宏、任意宏配置或源码 typedef 链。
- 读取器不支持完整 ECMA-335 签名语法，损坏输入可能触发 panic；使用受信任的 WinMD。
