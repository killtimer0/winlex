# windows-metadata 本地补丁

来源：[windows-metadata 0.100.0](https://crates.io/crates/windows-metadata/0.100.0)，上游仓库 [windows-rs](https://github.com/microsoft/windows-rs)。
原始 crate SHA256：`1d3a758dc1da4ae61aa7f93afbb505d564c72dd6431e513ed581377857896a72`。
MIT/Apache-2.0 许可证保留在源码目录中。Cargo 的 `[patch.crates-io]` 使用此副本。

本地副本补充了 Winlex 查询所需的读取能力：

- 字段显式布局及原始表枚举。
- 跨来源类型身份、引用作用域和程序集身份。
- 方法参数顺序及参数属性。
- 属性、事件与访问器关系。
- 类型和方法泛型参数及嵌套签名。

升级上游时，应检查这些扩展是否已有等价支持。尤其要保留 `TypeName::identity` 的身份与相等比较语义，或同步迁移调用方。

运行 `python tests/metadata/run.py`，使用独立 .NET 读取器及查询用例验证升级结果。环境准备和可选 SDK 验证见[构建与验证](../docs/build.md)。
