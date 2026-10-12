//! 子模块平表代际：`TableGen`（模块平表 + 每模块不可变常量缓存）及其表访问器，
//! 注册进 `Vm::tables` 按代际管理。

use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::Vm;

/// 单代子模块平表：模块平表 + 每模块不可变常量缓存槽位，注册进 `Vm::tables`
/// 按代际管理。
///
/// 以 `Box` 承载（HashMap 扩容搬 Box 指针不搬被指物）：`active_immutables` 与
/// `saved_immutables_stack` 是伸入模块级装载缓存条目常量 Vec 的胖指针，表地址须
/// 在其整个生命周期内稳定。动态扩表（`create_dynamic_function` /
/// `create_dynamic_script`）对 modules 外层 Arc 做 `make_mut`：彼时该 Arc 的
/// 强引用唯一持有者是本表（函数对象只持 u32 下标不持 Arc），原地扩展不分叉。
pub(crate) struct TableGen {
    /// 全局扁平模块表：下标 = 模块 `flat_id`（顶层 0，子模块 flatten 后全局唯一）。
    /// 函数对象 `sub_module_index` 即该平表下标，逃逸函数也能自足解析。
    pub(crate) modules: Arc<Vec<Arc<CompiledModule>>>,
    /// 不可变常量缓存槽位：下标 0 = 顶层模块，sub_idx = modules[sub_idx]。
    /// 每个 `OnceLock` 克隆模块级装载缓存条目（`Vm::module_cache`）中该模块
    /// 只转换一次的 `JsValue` 结果 Arc 入槽；槽位只作代际视图，转换本体归
    /// 缓存条目所有（跨 `full_reset` 存活）。不可变常量是标量 + perm 字符串 +
    /// 条目自持 BigInt，只读，GC 根收集按缓存条目遍历。
    pub(crate) immutables: Vec<OnceLock<Arc<Vec<JsValue>>>>,
    /// NEW_OBJECT 静态键 si 侧表槽位，与 `immutables` 平行（下标 = flat_id，
    /// 内部下标 = 常量池下标）：`Some(si)` 是模块装载期预 intern 的键 si，
    /// `None` 表示该常量不是静态键。填充与 `immutables` 激活同点
    /// （`activate_immutables`），构造期直读免逐构造键推导。
    pub(crate) si_tables: Vec<OnceLock<Arc<Vec<Option<u32>>>>>,
}

impl Vm {
    /// 活动模块已转换不可变常量的只读视图；任何 `run()` 之前为空。
    #[inline(always)]
    pub(crate) fn immutables(&self) -> &[JsValue] {
        if self.active_immutables.is_null() {
            &[]
        } else {
            // SAFETY: active_immutables 指向模块级装载缓存条目（module_cache）的
            // 常量 Vec，条目随 Vm 同消亡（跨 full_reset 存活），Vec 只填充一次且
            // 堆地址稳定。
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

    /// 激活代际 `gen` 平表中模块 `cache_idx` 的不可变常量，并把
    /// `active_immutables` 指向该模块已转换常量池。
    ///
    /// 转换本体归模块级装载缓存（`Vm::module_cache`，以模块 Arc 指针身份为键）
    /// 所有：命中时零转换、零字节码扫描、零 BigInt 分配；未命中时
    /// `convert_module_constants` 转换一次并填缓存。本方法只把条目 Arc 克隆入
    /// 该代际槽位（O(1)），槽位随代际回收，条目随 Vm 消亡。
    ///
    /// # 边界与前提
    /// - `gen` 的表须已在注册表中（当前代际由 run() 登记，旧代际由存活函数
    ///   对象引用保活）；`cache_idx` 须小于该代际平表长度。
    /// - `module` 是平表条目同一 Arc 实例（调用方克隆），作缓存键。
    pub(crate) fn activate_immutables(&mut self, gen: u32, cache_idx: usize, module: &Arc<CompiledModule>) {
        // 用裸指针访问缓存槽，避免 get_or_init（借用表 immutables）与查/建
        // module_cache 的借用发生冲突。成立前提：代际表归注册表所有、只读、
        // Box 承载地址稳定。
        let table = self.tables.get_mut(&gen).expect("激活的代际表须在注册表中");
        let slot: *const OnceLock<Arc<Vec<JsValue>>> = &table.immutables[cache_idx];
        // si 侧表槽位与 immutables 同点填充（条目共享同一份 si 表 Arc）。
        let si_slot: *const OnceLock<Arc<Vec<Option<u32>>>> = &table.si_tables[cache_idx];
        let key = Arc::as_ptr(module);
        let entry = match self.module_cache.get(&key) {
            Some(e) => e,
            None => {
                let (immutables, bigint_storage, si_table) = self.convert_module_constants(module);
                let new_entry = super::ModuleLoadEntry {
                    module: Arc::clone(module),
                    immutables,
                    bigint_storage,
                    si_table,
                };
                self.module_cache.entry(key).or_insert(new_entry)
            }
        };
        unsafe { &*slot }.get_or_init(|| Arc::clone(&entry.immutables));
        unsafe { &*si_slot }.get_or_init(|| Arc::clone(&entry.si_table));
        self.active_immutables = entry.immutables.as_slice() as *const [JsValue];
    }

    /// 填充模块的 NEW_OBJECT 静态键 si 侧表：按 dispatch 主循环同口径
    /// （`ext_word_count` 逐指令推进）扫字节码，收集每条带键表 NEW_OBJECT 的
    /// 键常量池下标，按键值经 `property_key_si` 预推导 si 存入侧表
    /// （下标 = 常量池下标），其余常量保持 None。
    ///
    /// # 边界与前提
    /// - 键值取自已转换的不可变常量（`values`），与构造期读同一池。
    /// - 键推导失败（非字符串键的防御面）保持 None，构造期回退旧路径同口径重推。
    pub(crate) fn fill_si_table(&mut self, values: &[JsValue], bytecode: &[opcode::Instr]) -> Vec<Option<u32>> {
        let mut si = vec![None; values.len()];
        let mut i = 0;
        while i < bytecode.len() {
            let instr = bytecode[i];
            if opcode::opcode(instr) == opcode::OpCode::NEW_OBJECT && opcode::a(instr) > 0 {
                let nprops = opcode::a(instr) as usize;
                for &w in &bytecode[i + 1..i + 1 + nprops] {
                    let idx = w as usize;
                    if idx < si.len() && si[idx].is_none() {
                        if let Ok(key_si) = self.property_key_si(values[idx]) {
                            si[idx] = Some(key_si);
                        }
                    }
                }
            }
            i += 1 + opcode::ext_word_count(bytecode, i);
        }
        si
    }

    /// 当前活动字节码的可变访问入口。bytecode 以 `Arc<[Instr]>` 与代际表 modules 源共享，
    /// IC 写回经 `Arc::make_mut` 保证独占：独占时零拷贝原地写，共享时先深拷贝再写
    /// （IC miss 才触发，频率低）。所有写操作必须经此方法，防止共享缓冲被多实例污染。
    pub(crate) fn bytecode_mut(&mut self) -> &mut [opcode::Instr] {
        Arc::make_mut(&mut self.bytecode)
    }
}
