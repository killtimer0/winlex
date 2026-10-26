# windows-metadata 本地补丁

来源：[windows-metadata 0.100.0](https://crates.io/crates/windows-metadata/0.100.0)，上游仓库 [windows-rs](https://github.com/microsoft/windows-rs)。
原始 crate SHA256：`1d3a758dc1da4ae61aa7f93afbb505d564c72dd6431e513ed581377857896a72`。
MIT/Apache-2.0 许可证保留在源码目录中。Cargo 的 `[patch.crates-io]` 使用此副本。

主要扩展：

- 保留 `FieldLayout` 表，提供 `Field::explicit_offset()`。
- `Index::rows()`、`File::row_count()` 公开原始表枚举。
- `TypeName::identity` 保留文件序号和 ECMA token；名称改写时清空失效身份。
- `TypeRef::scope_token()` 暴露 ResolutionScope。
- 保留 `AssemblyRef` 表，公开程序集名称、版本、culture、flags 和公钥/token；版本按四个 u16 读取，避免 x86 截断。
- 方法参数按 sequence 映射，提供方向、可选标记及缓冲区属性读取。
- 保留 Property、PropertyMap、Event、EventMap、MethodSemantics 表，公开属性/事件及其 accessor 关系。
- 保留类型与方法泛型参数的不同作用域；读取方法泛型参数数量，展开具名类型的泛型参数签名，支持数组参数嵌套。

`TypeName` 的构造及相等比较包含新增 identity 字段。升级上游时须保留这些语义或迁移调用方。
TypeSpec、custom modifier、多维数组和任意函数指针仍受上游解析能力限制。

使用 `python verify/run.py` 对照独立 .NET 读取器并验证显式偏移、声明大小、程序集身份及跨来源布局、COM 继承和反向引用。
