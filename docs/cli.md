# CLI 与查询接口

```text
winlex [选项] 命令 [查询]
```

输入查找顺序：`--winmd PATH`、`WINLEX_WINMD` 环境变量、程序旁的 `Windows.Win32.winmd`。

## 命令

| 命令 | 用途 |
|---|---|
| `summary` | 输入哈希及表行数 |
| `search TEXT` | 名称子串搜索；支持 `--exact`、`--prefix` |
| `value INTEGER` | 十进制或 `0x` 十六进制整数搜索 |
| `guid GUID` | GUID 搜索，允许外层花括号 |
| `show NAME或TOKEN` | 类型、字段、方法或类型引用详情 |
| `layout TYPE --arch ARCH` | 大小、对齐、字段和位域偏移 |
| `vtable TYPE --arch ARCH` | 继承后的 COM 方法槽位 |
| `references TYPE --arch ARCH` | 字段/方法签名和接口基类中的反向引用 |

`ARCH` 可选 `x86`、`x64`、`arm64`。搜索分页使用 `--limit`（默认50，上限1000）和 `--offset`。
`--help`、`--version` 单独调用。

搜索不区分大小写。声明选择优先匹配精确大小写，再尝试忽略大小写；`POINT` 和 GdiPlus 的 `Point` 是不同类型。
存在多个候选时，使用架构、限定名或 token 消除歧义。限定名形如 `Namespace.Type.Field`，嵌套类型形如 `Namespace.Outer+Inner`。
Token 仅在对应 WinMD 内有效。

## 输出

默认输出文本；`--json` 输出一份完整 JSON 至 stdout：

```json
{"schema":"winlex-cli-1","ok":true,"source_sha256":"...","command":"show","result":{}}
```

失败时：

```json
{"schema":"winlex-cli-1","ok":false,"error":{"code":"query_error","message":"..."}}
```

退出码：0成功、2参数错误、3输入或查询失败、4输出失败。文本模式错误写入 stderr。

- 整数以十进制字符串和保留位宽的十六进制表示；数值搜索按数学整数比较，`-1` 不等于 `0xffffffff`。
- 类型签名保留具名引用的 `reference_token`，可用 `show TOKEN --arch ARCH` 继续导航。
- 布局单位为 bit，偏移相对直接包含它的聚合；位域另提供相对底层字段的偏移。
- vtable 同时提供槽位和字节偏移。

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

## 限制

- 只解析单个 WinMD，不解析外部程序集；未解析引用会显式报告。
- 非零显式偏移、显式大小和灵活数组等尚不支持布局计算。
- 反向引用不包含属性字符串、宏或文档关系。
- 不提供完整文档正文、函数宏、任意宏配置或源码 typedef 链。
- 读取器不支持完整 ECMA-335 签名语法，损坏输入可能触发 panic；使用受信任的 WinMD。
