//! emit 前置 pass：builtin 引用预扫描 + 声明预登记。
//!
//! 在生成临时寄存器前遍历 AST，把内置全局标识符预先登记到固定寄存器槽
//! （builtin_reg_map），避免与临时寄存器池冲突；同时预声明函数/var 声明，
//! 支持提升语义。

mod block_fn;
mod builtin_scan;
mod declare;
