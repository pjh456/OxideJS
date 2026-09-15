//! 类编译域：class 声明的整体 emit 拆分为头/原型/方法/字段等子模块。

pub mod emit;
pub mod field;
pub mod header;
pub mod method;
pub mod prototype;
