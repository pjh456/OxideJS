//! test262 测试文件头部 YAML 元数据结构（`TestMeta`/`Negative`）与解析剥离函数（`parse_meta`/`strip_meta`）。
//! 结构与函数放宽为 `pub(crate)` 供 crate 内跨模块访问；YAML 解析走全限定 `serde_yaml::from_str`。

use serde::Deserialize;

/// 测试头部 YAML 元数据中的 `negative` 段：声明期望的失败阶段与错误类型。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct Negative {
    pub(crate) phase: String,
    #[serde(rename = "type")]
    pub(crate) error_type: String,
}

/// test262 测试文件头部 `/*--- ... ---*/` 段解析出的元数据
/// （description / flags / includes / features / negative 等）。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct TestMeta {
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) flags: Vec<String>,
    #[serde(default)]
    pub(crate) includes: Vec<String>,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) negative: Option<Negative>,
    #[serde(default)]
    pub(crate) es5id: String,
    #[serde(default)]
    pub(crate) es6id: String,
    #[serde(default)]
    pub(crate) esid: String,
}

/// 从测试源码头部解析 `/*--- YAML ---*/` 元数据；无该头部返回 None。
pub(crate) fn parse_meta(source: &str) -> Option<TestMeta> {
    let header_start = source.find("/*---")?;
    let header = &source[header_start..];
    let header = header.strip_prefix("/*---")?;
    let end = header.find("---*/")?;
    let yaml_body = &header[..end];
    let yaml_body = yaml_body.trim();
    serde_yaml::from_str::<TestMeta>(yaml_body).ok()
}

/// 剥离测试源码头部的 YAML 元数据段，返回纯 JS 代码。
pub(crate) fn strip_meta(source: &str) -> &str {
    if let Some(pos) = source.find("---*/") {
        return source[pos + 5..].trim_start();
    }
    source
}
