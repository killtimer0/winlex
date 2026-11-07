# windows-metadata 本地补丁

来源：[windows-metadata 0.100.0](https://crates.io/crates/windows-metadata/0.100.0)，上游仓库 [windows-rs](https://github.com/microsoft/windows-rs)。
原始 crate SHA256：`1d3a758dc1da4ae61aa7f93afbb505d564c72dd6431e513ed581377857896a72`。
MIT/Apache-2.0 许可证保留在源码目录中。Cargo 的 `[patch.crates-io]` 使用此副本。

本地副本扩展了布局、跨来源类型身份、方法参数、属性和事件及泛型签名的读取能力。升级上游时应检查是否已有等价支持，并保留 `TypeName::identity` 的身份与相等比较语义，或同步迁移调用方。

基础类型识别集中在 `reader/foundation.rs`，按限定名与核心程序集身份区分基类标记、特性编码和签名修饰符；支持的身份范围以该实现为准。程序集定义的强名称 token 使用已锁定的 SHA-1 依赖计算，仅用于身份比较，不校验签名真实性。GUID 的原生 ABI 由 Winlex 布局层提供，不读取参考程序集的实例字段。

运行 `python tests/metadata/run.py`，使用独立 .NET 读取器及查询用例验证升级结果。环境准备和可选 SDK 验证见[构建与验证](../docs/build.md)。
