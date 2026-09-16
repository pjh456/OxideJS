//! 子模块平表代际：`TableGen`（模块平表 + 每模块不可变常量缓存）及其表访问器，
//! 注册进 `Vm::tables` 按代际管理。

use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::Vm;

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

impl Vm {
    /// 活动模块已转换不可变常量的只读视图；任何 `run()` 之前为空。
    #[inline(always)]
    pub(crate) fn immutables(&self) -> &[JsValue] {
        if self.active_immutables.is_null() {
            &[]
        } else {
            // SAFETY: active_immutables 指向某表代际 immutables 内 OnceLock 的常量
            // Vec，Vec 只填充一次且堆地址稳定；所指代际表在注册表内存活（当前
            // 代际恒在，旧代际由存活函数对象引用保活）。
            unsafe { &*self.active_immutables }
        }
    }

    /// 当前代际的子模块平表（可变，动态扩表与测试注入的目标）。
    pub(crate) fn current_table_mut(&mut self) -> &mut TableGen {
        self.tables.get_mut(&self.current_gen).expect("当前代际表构造期已预登记")
    }

    /// 当前执行帧所属代际的子模块平表：按帧切换时记录的 `active_table_gen`
    /// 解析（压帧 / inline 入口 / run 顶层 / 挂起恢复置位，弹帧还原），跨 run
    /// 调用时可异于 `current_gen`——帧内指令操作数是帧字节码所属代际平表里的
    /// flat_id，须按该代际解析。
    ///
    /// # 边界与前提
    /// - 执行帧代际表恒在注册表中：顶层即当前代际（恒保留），帧 / inline 的
    ///   代际由存活 callee 函数对象保活，挂起恢复已按 callee 对象验证在位。
    pub(crate) fn active_table(&self) -> &TableGen {
        self.tables.get(&self.active_table_gen).expect("执行帧的代际表须在注册表中")
    }

    /// 函数对象 `sub_module_index` 指向的子模块条目：按对象自身记录的表代际
    /// 解析平表后按下标定位。
    ///
    /// # 边界与前提
    /// - 原生函数哨兵 `sub_module_index() == 0` 不经本方法（调用方先行分派）；
    /// - 代际表已被回收或下标越界返回 None（调用方按各自越界口径报错）。
    pub(crate) fn callee_module<'a>(&'a self, obj: &'a JsObject) -> Option<&'a CompiledModule> {
        self.tables
            .get(&obj.table_gen())
            .and_then(|t| t.modules.get(obj.sub_module_index() as usize))
            .map(|m| m.as_ref())
    }

    /// 活动帧所属代际平表里 `active_flat_id` 指向的模块条目（镜像槽同步 /
    /// 重载的名集载体）。
    ///
    /// # 边界与前提
    /// - 活动代际表在执行期恒在注册表中（首 run 前为预登记的空表占位），
    ///   `active_flat_id` 越界返回 None（调用方按无模块处理）。
    pub(crate) fn active_module(&self) -> Option<&CompiledModule> {
        self.active_table()
            .modules
            .get(self.active_flat_id as usize)
            .map(|m| m.as_ref())
    }

    /// 子模块表注册表的当前代际条目总数（run 边界回收行为的测试钉）。
    pub fn table_gen_count(&self) -> usize {
        self.tables.len()
    }

    /// 激活代际 `gen` 平表中模块 `cache_idx` 的不可变常量，只转换一次存入该代际
    /// `immutables[cache_idx]`，并把 `active_immutables` 指向该 Vec。
    /// `constants` 由调用方传入（它已持有 `&module.constants`）。
    ///
    /// # 边界与前提
    /// - `gen` 的表须已在注册表中（当前代际由 run() 登记，旧代际由存活函数
    ///   对象引用保活）；`cache_idx` 须小于该代际平表长度。
    pub(crate) fn activate_immutables(&mut self, gen: u32, cache_idx: usize, constants: &[Constant]) {
        // 用裸指针访问缓存槽，避免 get_or_init（借用表 immutables）与 &self 的
        // convert_immutables 闭包和随后对 active_immutables 的写入发生借用冲突。
        // 成立前提：代际表归注册表所有、只读、Box 承载地址稳定。
        let table = self.tables.get_mut(&gen).expect("激活的代际表须在注册表中");
        let slot: *const OnceLock<Vec<JsValue>> = &table.immutables[cache_idx];
        let vec = unsafe { &*slot }.get_or_init(|| self.convert_immutables(constants));
        self.active_immutables = vec.as_slice() as *const [JsValue];
    }

    /// 当前活动字节码的可变访问入口。bytecode 以 `Arc<[Instr]>` 与代际表 modules 源共享，
    /// IC 写回经 `Arc::make_mut` 保证独占：独占时零拷贝原地写，共享时先深拷贝再写
    /// （IC miss 才触发，频率低）。所有写操作必须经此方法，防止共享缓冲被多实例污染。
    pub(crate) fn bytecode_mut(&mut self) -> &mut [opcode::Instr] {
        Arc::make_mut(&mut self.bytecode)
    }
}
