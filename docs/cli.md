# CLI 使用指南

```text
winlex FILE... 命令 [查询] [选项]
```

元数据命令的输入文件放在命令前；`--help`、`--version` 和独立的 `error-code` 命令无需输入文件。文件名与命令同名时使用带目录的路径，例如 `.\search`；以 `--` 开头的文件名也使用带目录的路径。
依次列出文件路径可加载多个文件；来源按输入顺序编号，从 0 开始。

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
| `iid TYPE --arch ARCH` | WinRT 接口或委托的 IID，支持闭合泛型实例 |
| `references TYPE --arch ARCH` | 字段、方法、属性、事件签名及接口要求中的反向引用 |
| `error-code CODE` | 独立查询错误码或模块消息，不加载 WinMD |
| `demangle SYMBOL...` | 还原 Visual C++ 与 Itanium 修饰名，不加载 WinMD |
| `com TARGET` | 从本机注册表解析 COM/WinRT 的 ProgID、CLSID、IID、LIBID 与 AppID，不加载 WinMD |

`ARCH` 可选 `x86`、`x64`、`arm64`，与程序自身架构无关。搜索和反向引用使用 `--limit`（默认 50，范围 0–1000）和 `--offset` 分页。

## 搜索

搜索不区分大小写，声明名称选择区分大小写。`--name`、`--value`、`--guid`、`--where` 互斥，且仅用于 `search`。
快速搜索和 `--name` 在查询不含 `.` 或 `+` 时匹配短名称，否则匹配完整限定名；末尾 `.`／`+` 可浏览命名空间后代或嵌套类型。

- 快速搜索同时尝试名称模糊匹配、数字等值、字符串常量子串和 GUID，每个实体只返回一次。GUID 查询须在去除花括号、连字符或转换字节列表后包含至少 4 个十六进制字符；GUID 专用模式允许更短的片段。普通文本不解释为条件表达式，空格不表示 AND。
- `--name` 精确匹配名称，不做模糊匹配。
- 快速搜索的模糊匹配允许字符不连续，但须保持顺序；标点按字面匹配，不忽略重音。
- `--value` 接受带正负号的二进制 `0b`、前导零八进制、`0o`、十进制和 `0x` 整数。按能容纳输入的 8/16/32/64 位解释位模式，不截断；同一位宽可重解释符号。例如 `257` 不匹配 8 位的 `1`，`0xffffffff` 匹配 32 位的 `-1`，但不匹配 64 位的 `-1`。
- 含小数点或指数时搜索浮点常量，按常量声明类型转换后比较，`1.5` 和 `1.50` 等价。整数语法不搜索浮点值。`nan` 匹配任意 NaN，`inf`／`+inf` 匹配正无穷，`-inf` 匹配负无穷。不区分大小写，正负零相等。
- `--guid` 接受完整 GUID（可带花括号）或连续十六进制片段，保留前导零，可跨 GUID 分组匹配。字节顺序按显示的 GUID 文本；`239,254` 表示 `effe`，`FF 0A` 表示 `ff0a`，`0xBC,0xAB` 表示 `bcab`。无前缀数字列表为十进制；出现无前缀的 A–F 时，无前缀项均须是两位十六进制字节。

快速搜索优先返回精确命中，其次为 GUID 片段、文本匹配和模糊名称；模糊名称按匹配度排列。

## 选择声明与来源

存在多个候选时，使用架构、限定名或 token 消除歧义。限定名形如 `Namespace.Type.Field`，嵌套类型形如 `Namespace.Outer+Inner`。
选择声明时支持 `SOURCE::NAME` 和 `SOURCE::0xTOKEN`，SOURCE 可用来源编号或唯一程序集名称（包括名称中的 `.winmd` 后缀）。裸 token 仅允许单来源。来源编号取决于本次输入顺序，token 仅标识所属文件中的记录。
`--source SOURCE` 在搜索时过滤来源，在 show/layout/vtable/iid/references 时限定目标来源，不能与目标根部的 `SOURCE::` 同用；实参可单独指定来源。反向引用仍查找所有已加载来源。独立来源中的同名声明不会自动合并。
跨文件引用所需的依赖须显式加载；非 WinRT 引用要求程序集身份匹配。

受支持核心程序集中的基础引用无需额外加载 CLR 程序集，身份范围与上下文语义见 [foundation.rs](../vendor/windows-metadata/src/reader/foundation.rs)。`System.Guid` 的原生大小为 16 字节、对齐 4 字节，加载 CLR 参考程序集不会改变该 ABI。

## 泛型实例

`show`、`iid`、`vtable` 接受 `类型<实参,...>`，实例查询需指定 `--arch`。类型名区分大小写；泛型名称可省略元数据中的反引号参数数量后缀。

```powershell
winlex Windows.winmd show 'Windows.Foundation.Collections.IVector<int32>' --arch x64
winlex Windows.winmd iid 'Windows.Foundation.Collections.IVector<int32>' --arch x64
winlex Windows.winmd vtable 'Windows.Foundation.Collections.IMap<string, object>' --arch x86 --json
```

实参支持闭合命名类型、嵌套实例，以及 `bool`、`char`、`uint8`、`int16`、`uint16`、`int32`、`uint32`、`int64`、`uint64`、`nint`、`nuint`、`float32`、`float64`、`string`、`object`、`guid`。同名类型须使用限定名或来源（如 `0::Namespace.Type`）。`[]` 数组、`int8` 和 `nint`／`nuint` 可用于声明浏览，但不能作为 WinRT IID 的实参。这些基元名称同时用于声明正文、成员类型行、JSON 的 `type`／`name` 和常量标注；`void` 只出现在返回类型中。

`iid` 返回实例 IID 和规范 WinRT 类型签名；实例 `show --json` 保留真实定义 ID，在 `type_arguments` 中单独列出实参。CLI 搜索仍查找元数据定义，不构造实例。

泛型 WinRT `vtable` 返回隐式基接口槽位、方法索引和目标架构下的字节偏移。实例结果另含 IID 与逐方法的标量 ABI：返回 `HRESULT`，元数据返回值变为最后一个 `[out, retval]` 参数，字符串使用 `HSTRING`，接口／委托指针按 IID 标识。x86 使用 `stdcall`，x64／ARM64 使用平台调用约定。
这是参数层面的 ABI 阅读视图，不描述寄存器分配或完整可编译声明。数组、结构体、运行时类及开放／方法泛型的 ABI 展开不支持；不支持的方法仍保留槽位，并给出原因，不输出猜测的 ABI。泛型值类型布局及 CLR 约束检查不支持。

## 错误码查询

```powershell
winlex error-code 5 --scope win32
winlex error-code -2147024891 --scope hresult --lang 0x409
winlex error-code 0xC0000005 --scope ntstatus --json
winlex error-code 2 --scope errno
winlex error-code 0x20000001 --module .\messages.dll --lang auto
```

- `--scope` 可选 `auto`（默认）、`win32`、`hresult`、`ntstatus`、`errno`。自动模式分别查询 Win32、HRESULT 和 NTSTATUS，不查询 CRT errno。
- `errno` 查询链接的 Microsoft CRT 错误表（x86／x64 为 MSVCRT，ARM64 为 UCRT），JSON 来源标识为 `crt`。消息固定为英文；`--lang auto`、`0` 或 `0x409` 均可，其他 LANGID 返回语言未找到。
- `--module PATH` 指定单个 EXE、DLL 或 MUI 的消息表，与预设 `--scope` 互斥；使用完整消息 ID，不回退到系统消息表。
- `--lang auto|LANGID`（也可写 `--language`）选择消息语言。除上述 CRT errno 外，`auto` 或 `0` 使用 Windows 的语言选择规则；非零 LANGID 严格请求对应语言。
- 代码接受 -2147483648 至 4294967295，支持正负号、0x／0o／0b 前缀；前导零为八进制，例如 `010` 为 8。LANGID 为无符号 16 位值。不接受符号名称或浮点数。
- HRESULT 可附带关联的 Win32 错误。NTSTATUS 和自定义模块保留消息模板中的占位符，并报告资源语言；系统消息自动选择时实际语言未知。
- `--json` 输出版本化结构化结果。没有找到消息仍退出 0；参数错误退出 2，执行故障退出 3，输出失败退出 4。部分来源失败时保留其它结果并返回 3。
- 本机系统版本和资源语言决定可查询的消息范围。

## 符号还原

```powershell
winlex demangle '?CreateFileW@@YAPEAXPEBGKKPEAU_SECURITY_ATTRIBUTES@@W4_CREATIONDISPOSITION@@W4_FILE_FLAGS_AND_ATTRIBUTES@@PEAX@Z'
winlex demangle '_ZN3Foo3barEi' '_Z3fooi' --abi itanium
winlex demangle --file .\exports.txt --json
```

- 仅做单向 demangle，不生成修饰名，也不读取 DLL／EXE 导出表。
- `--abi auto`（默认）按前缀判断：`?` 为 Visual C++，`_Z` 为 Itanium，`_name@N`／`@name@N` 为 x86 C 装饰。无法判断时要求显式指定 ABI。
- Visual C++ 结果包含返回类型和调用约定；Itanium 普通函数不编码返回类型，结果只描述已编码的信息。
- Rust v0（`_R…`）符号不支持，直接报错；形如 `_name@N` 的 x86 C 装饰仍按 C 装饰解析。
- `--file` 读取 UTF-8 文本，每行一个符号，空行忽略；可与位置参数同时使用，文件内容在前。
- 文本输出每行 `原始 -> 还原 [abi, 类型(, 调用约定)]`；解析失败的符号保留原行并输出错误。
- `--json` 输出版本化结果，每个符号包含 `symbol`、`ok`，成功时带 `result`（`text`、`abi`、`kind`、`convention`、`spans`），失败时带 `error`。
- `spans` 使用 UTF-16 偏移标注可着色名称区域；当前只标注函数／成员名与限定名，未识别的片段不着色。
- 任一符号还原失败时退出 3；参数错误退出 2；输出失败退出 4。

## COM / WinRT 注册查询

```powershell
winlex com "Shell.Application"
winlex com "{00021401-0000-0000-C000-000000000046}"
winlex com "{00020400-0000-0000-C000-000000000046}" --view 32
winlex com "MyLib.MyClass" --kind progid --json
```

- 只读取本机注册表，与已加载元数据无关；不需要输入文件。
- `--kind` 可选 `auto`（默认）、`progid`、`guid`。自动模式先按 GUID 解析，再按 ProgID 识别；无法判断时报错。
- `--view` 默认 `both`，同时查询 32 位与 64 位注册表视图；`auto` 使用当前进程视图。系统不支持的视图会给出警告。
- 同时读取机器级（HKLM）与用户级（HKCU）注册表。相同值只显示一次；不同视图或来源的值分行显示，并标注 `x86 machine`、`x64 user` 等来源。
- GUID 会依次检查 CLSID、Interface、TypeLib、AppID，多处命中时全部列出；ProgID 会解析其 CLSID 并继续展开被引用的类型库和 AppID。
- 文本输出按输入／目标 → ProgID → CLSID → Interface → TypeLib → AppID → 警告排列；类型库按版本和语言分组。
- `REG_EXPAND_SZ` 按当前进程环境变量展开。
- `--json` 输出版本化结构，包含 `kind`、`normalized`、`groups`（`kind`、`identity`、`fields`、`children`）和 `warnings`；字段值带 `sources`（`view`、`origin`）。
- 无法识别输入时退出 3；参数错误退出 2；输出失败退出 4。

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
| `category` | `=` 配合 `"type"`、`"function"`、`"method"`、`"constant"`、`"field"`、`"property"`、`"event"`、`"type_reference"` |
| `source` | `= 0` 等从零开始的输入编号 |
| `architecture` | `= "x86"`、`= "x64"`、`= "arm64"`，表示适用于该架构，包含共有定义 |

- `NOT > AND > OR`，可用括号分组；条件之间必须写逻辑操作符。关键字不区分 ASCII 大小写。
- 字符串加双引号，支持 `\"`、`\\`、`\n`、`\r`、`\t`、`\0` 转义，也可直接输入 Unicode。
- `value = 5`、`value = 5.0`、`value = "5"` 分别查询整数、浮点数和字符串。
- 整数范围按声明的有符号／无符号数值比较；整数等值仍使用上述位宽规则。
- 浮点边界按目标类型舍入，极近的边界可能变成同一个值。NaN 不能作为有序边界，候选 NaN 不匹配任何范围。无穷可作边界，`value > -inf and value < inf` 匹配有限浮点值。
- 缺少字段的条目不满足该字段条件；`not (value = 5)` 也包含没有常量值的条目。只查询其他常量时再加 `category = "constant"`。
- `--source`、`--arch` 与表达式取交集。字符串搜索限于输入元数据中实际保留的值。

## 输出

默认输出文本；`--json` 输出一份完整 JSON 至 stdout：

```json
{"schema":"winlex-cli-1","ok":true,"sources":[],"command":"show","result":{}}
```

失败时：

```json
{"schema":"winlex-cli-1","ok":false,"error":{"code":"query_error","message":"..."}}
```

退出码：0成功、2参数错误、3输入或查询失败、4输出失败。文本模式错误写入 stderr。

- 整数以十进制字符串和保留位宽的十六进制表示。
- 搜索条目的 `evidence` 提供命中字段和 Unicode 字符索引；`indices: null` 表示整个值。条件错误提供 UTF-8 字节范围 `span` 和是否尚未完整输入的 `incomplete`。
- 布局、vtable 和反向引用结果使用 `architecture` 字段，值为 `x86`、`x64` 或 `arm64`。
- `sources` 提供来源编号、路径、程序集身份和依赖。实体 ID 为 `{"source_id":0,"token":33554433}`，token 使用数值表示。
- 实体 `kind` 区分三种方法形态：静态方法带 ImplMap，或实现标志为 Native|Unmanaged 的内联声明，为 `function`；其余静态方法为 `static_method`；实例方法为 `method`。内联声明没有导入记录，`show --json` 的 `import` 为 `null`。`category` 仍只有 `function` 与 `method` 两个过滤位，`category = "method"` 同时包含 `static_method`。
- 类型签名保留具名引用的 `reference_id`，解析后的 TypeRef 提供 `target_id`；可转换为 `SOURCE::0xTOKEN` 继续查询。
- TypeRef 的 `resolution` 区分已加载的实际定义 `target_id`、由读取器直接解释的内建语义 `builtin`／`semantics` 和未解决依赖 `unresolved`。
- 布局单位为 bit，偏移相对直接包含它的聚合；位域另提供相对底层字段的偏移。
- vtable 同时提供槽位和字节偏移。

## WinRT 与 Windows App SDK

可加载 Windows SDK 合并版 `Windows.winmd`，或依次列出文件路径加载分立 contracts。Windows App SDK 的 WinMD 位于各组件 NuGet 包的 `metadata/` 下；所需依赖也须显式加载。

- WinRT 引用按类型全名匹配已加载的 WinRT 定义。合并版与分立 contracts 应选择一套，重叠定义会导致歧义。
- `show` 可查看类型和方法的泛型参数、实例化参数、属性、事件及访问器，以及基类和默认接口。runtime class 的静态、激活和组合工厂可在 `StaticAttribute`、`ActivatableAttribute`、`ComposableAttribute` 中查看。
- 非泛型 WinRT 接口的 vtable 包含 `IUnknown`、`IInspectable` 六个基础槽位；委托包含 `IUnknown` 三槽及 `Invoke`。隐式槽位的实体 ID 为 null。
- `show` 提供元数据签名；WinRT 方法 ABI 签名展开尚不支持。

## 查询范围

声明和类型引用来自输入 WinMD 中保留的元数据。

- 开放泛型的布局/vtable、泛型实例 ABI 及部分 WinRT 字段类型（例如 HSTRING）尚不支持计算。
- 暂不支持 ModuleRef、类型转发和 retargetable 程序集引用。
- 非零显式偏移、显式大小和灵活数组等尚不支持布局计算。

损坏的 WinMD 输入可能导致程序异常退出。
