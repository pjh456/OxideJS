//! Intl 特性域：命名空间对象、十组构造器/原型对、`getCanonicalLocales` 与
//! Locale 构造器（骨架阶段）。
//!
//! 规范化映射表与 CLDR 36.1 逐位一致（test262 harness 同源），结构校验遵循
//! UTS35（不接受 BCP47 宽松形式）。真实格式化语义（format / formatToParts /
//! resolvedOptions / compare 等）由后续任务填充。

mod cldr;
mod native;
mod tables;

pub use native::*;
