# CLI 与查询接口

```text
winlex FILE... 命令 [查询] [选项]
```

元数据命令的输入文件放在命令前；`--help`、`--version` 和独立的 `error-code` 命令无需输入文件。文件名与命令同名时使用带目录的路径，例如 `.\search`；以 `--` 开头的文件名也使用带目录的路径。
依次列出文件路径可加载多个文件；来源按输入顺序编号，从 0 开始，重复输入保留独立条目。

```powershell
winlex Windows.Win32.winmd Windows.Wdk.winmd summary
winlex Windows.Win32.winmd Windows.Wdk.winmd show Windows.Wdk.winmd::NtCancelIoFileEx --arch x64
```

## 命令

| 命令 | 用途 |
|---|---|
| `summary` | 来源、程序集身份及表行数 |
| `search TEXT` | 快速搜索名称、常量值和 GUID；可用 `--name`、`--value` 或 `--guid` 限定字段 |
| `search --where EXPR` | 按字段和逻辑关系组合条件 |
| `show NAME或ID` | 类型、成员或类型引用详情 |
| `layout TYPE --arch ARCH` | 大小、对齐、字段和位域偏移 |
| `vtable TYPE --arch ARCH` | COM、WinRT 接口及委托的方法槽位 |
| `references TYPE --arch ARCH` | 字段、方法、属性、事件签名及接口要求中的反向引用 |
| `error-code CODE` | 独立查询错误码或模块消息，不加载 WinMD |

`ARCH` 可选 `x86`、`x64`、`arm64`。搜索分页使用 `--limit`（默认50，上限1000）和 `--offset`。
`--help`、`--version` 单独调用。

搜索不区分大小写，声明名称选择区分大小写。`--name`、`--value`、`--guid`、`--where` 互斥，且仅用于 `search`。

- 快速搜索同时尝试名称模糊匹配、数字等值、字符串常量子串和 GUID，每个实体只返回一次。GUID 查询须在去除花括号、连字符或转换字节列表后包含至少 4 个十六进制字符；GUID 专用模式允许更短的片段。普通文本不解释为条件表达式，空格不表示 AND。
- `--name` 精确匹配名称；以 `.` 结尾浏览命名空间后代，以 `+` 结尾浏览嵌套类型。查询不含 `.` 或 `+` 时匹配短名称，否则匹配完整限定名。
- 快速搜索的模糊匹配允许字符不连续，但须保持顺序；标点按字面匹配，不忽略重音。
- `--value` 接受带正负号的二进制 `0b`、前导零八进制、`0o`、十进制和 `0x` 整数。按能容纳输入的 8/16/32/64 位解释位模式，不截断；同一位宽可重解释符号。例如 `257` 不匹配 8 位的 `1`，`0xffffffff` 匹配 32 位的 `-1`，但不匹配 64 位的 `-1`。
- 含小数点或指数时搜索浮点常量，按常量声明类型转换输入后比较，`1.5` 和 `1.50` 等价。整数语法不搜索浮点值。`nan` 匹配任意 NaN，`inf`／`+inf` 匹配正无穷，`-inf` 匹配负无穷，关键字不区分 ASCII 大小写。正负零相等。有限字面量溢出或非零值舍入为零时，不匹配对应浮点类型；所有浮点类型均不可表示时报告输入错误。
- `--guid` 接受完整 GUID（可带花括号）或连续十六进制片段，保留前导零，可跨 GUID 分组匹配。字节顺序按显示的 GUID 文本；`239,254` 表示 `effe`，`FF 0A` 表示 `ff0a`，`0xBC,0xAB` 表示 `bcab`。无前缀数字列表为十进制；出现无前缀的 A–F 时，无前缀项均须是两位十六进制字节。

快速结果先排列精确名称、数字等值、字符串等值和完整 GUID，再排列 GUID 片段、文本包含／前缀与名称范围、模糊名称（按匹配度）。同级按来源编号、token 排序后分页。组合条件和反向引用按来源编号、token 排序；单个名称模糊条件按匹配度排序。
存在多个候选时，使用架构、限定名或 token 消除歧义。限定名形如 `Namespace.Type.Field`，嵌套类型形如 `Namespace.Outer+Inner`。
选择声明时支持 `SOURCE::NAME` 和 `SOURCE::0xTOKEN`，SOURCE 可用来源编号或唯一程序集名称（包括名称中的 `.winmd` 后缀）。裸 token 仅允许单来源。编号和 token 仅在所属 Catalog 内有效。
`--source SOURCE` 在搜索时过滤来源，在 show/layout/vtable/references 时限定目标来源；反向引用仍查找所有已加载来源。独立来源中的同名声明不会自动合并。
非 WinRT 引用按程序集名称、版本、culture、公钥 token 和 content type 匹配；依赖须显式加载。

## 错误码查询

```powershell
winlex error-code 5 --scope win32
winlex error-code -2147024891 --scope hresult --lang 0x409
winlex error-code 0xC0000005 --scope ntstatus --json
winlex error-code 0x20000001 --module .\messages.dll --lang auto
```

- `--scope` 可选 `auto`（默认）、`win32`、`hresult`、`ntstatus`。自动模式分别查询 Win32、HRESULT 和 NTSTATUS，并展示匹配结果。
- `--module PATH` 指定单个 EXE、DLL 或 MUI 的消息表，与预设 `--scope` 互斥；使用完整消息 ID，不回退到系统消息表。
- `--lang auto|LANGID`（也可写 `--language`）选择消息语言。`auto` 或 `0` 使用 Windows 的语言选择规则；非零 LANGID 严格请求对应语言。
- 代码接受 -2147483648 至 4294967295，支持正负号、0x／0o／0b 前缀；前导零为八进制，例如 `010` 为 8。LANGID 为无符号 16 位值。不接受符号名称或浮点数。
- 结果包含标准化代码、消息来源和语言信息。HRESULT 可附带关联的 Win32 错误。NTSTATUS 和自定义模块展示完整资源模板，保留 `%08lx`、`%s`、`%1!s!` 等占位符，并报告实际资源语言；系统消息自动选择时实际语言未知。
- `--json` 输出版本化结构化结果。没有找到消息仍退出 0；参数错误退出 2，执行故障退出 3，输出失败退出 4。部分来源失败时保留其它结果并返回 3。
- 本机系统版本和资源语言决定可查询的消息范围。

## 条件表达式

```powershell
winlex Windows.Win32.winmd search --where 'name contains "access" and value = 5'
winlex Windows.Win32.winmd search --where 'value contains "Windows"'
winlex Windows.Win32.winmd search --where 'value >= 1.45 and value < 1.55'
winlex Windows.Win32.winmd search --where 'value = nan or value = inf'
```

| 字段 | 操作与示例 |
|---|---|
| `name`、`fullname` | `=`、`contains`、`starts_with`、`fuzzy`，分别匹配短名和限定名 |
| `value` | 数字 `=`、`<`、`<=`、`>`、`>=`；字符串 `=`、`contains`、`starts_with` |
| `guid` | `=` 完整 GUID；`contains` GUID 片段或字节列表，值需加引号 |
| `namespace` | `=` 精确命名空间；`within` 包含当前及子命名空间 |
| `category` | `=` 配合 `"type"`、`"method"`、`"constant"`、`"field"`、`"property"`、`"event"`、`"type_reference"` |
| `source` | `= 0` 等从零开始的输入编号 |
| `architecture` | `= "x86"`、`= "x64"`、`= "arm64"`，表示适用于该架构，包含共有定义 |

- `NOT > AND > OR`，可用括号分组；条件之间必须写逻辑操作符。关键字不区分 ASCII 大小写。
- 字符串加双引号，支持 `\"`、`\\`、`\n`、`\r`、`\t`、`\0` 转义，也可直接输入 Unicode。
- `value = 5`、`value = 5.0`、`value = "5"` 分别查询整数、浮点数和字符串。
- 整数范围按声明的有符号／无符号数值比较；整数等值仍使用上述位宽规则。
- 浮点边界按目标类型舍入，极近的边界可能变成同一个值。NaN 不能作为有序边界，候选 NaN 不匹配任何范围。无穷可作边界，`value > -inf and value < inf` 匹配有限浮点值。
- 缺少字段的条目不满足该字段条件；`not (value = 5)` 也包含没有常量值的条目。只查询其他常量时再加 `category = "constant"`。
- `--source`、`--arch` 与表达式取交集。字符串搜索限于输入元数据中实际保留的值。
- 表达式最多 16384 UTF-8 字节，数字最多 4096 字节；嵌套和条件数量受限，过大输入会报错。

## 输出

默认输出文本；`--json` 输出一份完整 JSON 至 stdout：

```json
{"schema":"winlex-cli-7","ok":true,"sources":[],"command":"show","result":{}}
```

失败时：

```json
{"schema":"winlex-cli-7","ok":false,"error":{"code":"query_error","message":"..."}}
```

退出码：0成功、2参数错误、3输入或查询失败、4输出失败。文本模式错误写入 stderr。

- 整数以十进制字符串和保留位宽的十六进制表示。
- 搜索条目的 `evidence` 提供命中字段和 Unicode 字符索引；`indices: null` 表示整个值。条件错误提供 UTF-8 字节范围 `span` 和是否尚未完整输入的 `incomplete`。
- 布局、vtable 和反向引用结果使用 `architecture` 字段，值为 `x86`、`x64` 或 `arm64`。
- `sources` 提供来源编号、路径、程序集身份和依赖。实体 ID 为 `{"source_id":0,"token":33554433}`，token 使用数值表示。
- 类型签名保留具名引用的 `reference_id`，解析后的 TypeRef 提供 `target_id`；可转换为 `SOURCE::0xTOKEN` 继续查询。
- 布局单位为 bit，偏移相对直接包含它的聚合；位域另提供相对底层字段的偏移。
- vtable 同时提供槽位和字节偏移。

## WinRT 与 Windows App SDK

可加载 Windows SDK 合并版 `Windows.winmd`，或依次列出文件路径加载分立 contracts。Windows App SDK 的 WinMD 位于各组件 NuGet 包的 `metadata/` 下；所需依赖也须显式加载。

- WinRT 引用按类型全名匹配已加载的 WinRT 定义。合并版与分立 contracts 应选择一套，重叠定义会导致歧义。
- 声明保留类型/方法泛型参数、实例化参数、属性、事件和 accessor 方法 ID。`interface_implementations` 提供默认接口标记及关系属性，`base_type` 提供基类。
- runtime class 的静态、激活和组合工厂信息保留在 `StaticAttribute`、`ActivatableAttribute`、`ComposableAttribute` 中。
- 非泛型 WinRT 接口的 vtable 包含 `IUnknown`、`IInspectable` 六个基础槽位；委托包含 `IUnknown` 三槽及 `Invoke`。隐式槽位的实体 ID 为 null。
- `show` 提供元数据签名；WinRT 方法 ABI 签名展开尚不支持。

## Rust API

`winlex_core::Catalog` 提供同步查询，可通过 `Arc` 跨线程共享，结果拥有独立数据。
搜索和反向引用支持 `SearchOptions.cancel` 协作式取消；单次描述和布局查询不提供取消点。
`SearchOptions.mode` 选择 `SearchMode::{Combined, Name, Value, Guid, Condition}`，默认 `Combined`。无效输入返回带 `InvalidSearchInput` 上下文的错误；表达式错误可读取 `QueryError`。`Catalog::search_exact` 独立提供严格名称选择，不进行范围浏览。
可用 `Query::parse` 或构造 `Query` 条件树，再调用 `Catalog::search_query`；需要分离编译和执行时使用 `CompiledQuery` 与 `Catalog::search_compiled`。
搜索迭代器逐条返回 `SearchMatch`，其中 `entity` 是实体信息，`quality` 是排序质量，`evidence` 是实际命中字段。迭代顺序是扫描顺序，展示前使用 `SearchMatch::compare` 排序。
需要在未命中候选之间处理进度或发送缓冲时，可调用 `Search::step`，每次最多扫描一项候选，返回命中、跳过或结束状态；它与迭代器共享扫描位置和终止状态。

```rust,ignore
use winlex_core::{Architecture, Catalog, SearchMatch, SearchOptions};

let catalog = Catalog::open("Windows.Win32.winmd")?;
let mut matches = catalog.search("OPENFILE", SearchOptions::default())?
    .collect::<winlex_core::Result<Vec<_>>>()?;
matches.sort_unstable_by(SearchMatch::compare);
let target = catalog.search_exact("OVERLAPPED", SearchOptions {
    architecture: Some(Architecture::X64), ..Default::default()
})?
    .next().transpose()?.expect("OVERLAPPED").id;
let layout = catalog.layout(target, Architecture::X64)?;
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
