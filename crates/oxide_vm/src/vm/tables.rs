//! 子模块平表代际：`TableGen`（模块平表 + 每模块不可变常量缓存），注册进
//! `Vm::tables` 按代际管理。

use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;
use oxide_types::value::JsValue;

/// 单代子模块平表：模块平表 + 每模块不可变常量缓存，注册进 `Vm::tables` 按代际管理。
///
/// 以 `Box` 承载（HashMap 扩容搬 Box 指针不搬被指物）：`active_immutables` 与
/// `saved_immutables_stack` 是伸入 immutables 内部常量 Vec 的胖指针，表地址须
/// 在其整个生命周期内稳定。动态扩表（`create_dynamic_function` /
/// `create_dynamic_script`）对 modules 外层 Arc 做 `make_mut`：彼时该 Arc 的
/// 强引用唯一持有者是本表（函数对象只持 u32 下标不持 Arc），原地扩展不分叉。
pub(crate) struct TableGen {
    /// 全局扁平模块表：下标 = 模块 `flat_id`（顶层 0，子模块 flatten 后全局唯一）。
    /// 函数对象 `sub_module_index` 即该平表下标，逃逸函数也能自足解析。
    pub(crate) modules: Arc<Vec<Arc<CompiledModule>>>,
    /// 每次 run 转换一次的不可变常量缓存：下标 0 = 顶层模块，sub_idx = modules[sub_idx]。
    /// 每个 `OnceLock` 保存该模块常量本次运行中只转换一次的 `JsValue` 结果。
    /// 不可变常量是标量 + perm 字符串，只读，GC 根收集按表代际遍历。
    pub(crate) immutables: Vec<OnceLock<Vec<JsValue>>>,
}
