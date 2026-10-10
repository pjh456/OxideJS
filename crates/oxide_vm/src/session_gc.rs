use std::collections::HashSet;
use std::mem::size_of;
use std::time::Instant;

use crate::{vm_debug, vm_info};
use oxide_types::object::{Cell, JsObject, JsString, PropMetaEntry};
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::native_box_dispatch;
use crate::vm::RootGroup;
use crate::vm::Vm;
use oxide_builtins::{
    array_buffer, broadcast_channel, data_view, disposable_stack, event, map, message_channel, module, regexp, set,
    typed_array, weak_map,
};

/// session 级 mark-sweep GC 的状态与统计。
///
/// 回收 session 对象表中不再可达的对象与 `Vm::new_string` 分配的 session 字符串：
/// `mark` 从 VM roots 标记存活对象，`sweep` 原地清扫（存活对象带位保留、地址不变，
/// 死对象本体、堆区与独占 upvalue 列表原地释放出表）；`sweep_session_strings`
/// 按存活标记回收字符串。所有统计字段供外部观测 GC 行为。
pub struct SessionGc {
    pub total_collections: u64,
    pub total_bytes_freed: u64,
    pub total_objects_scanned: u64,
    pub total_objects_live: u64,
    pub total_objects_dead: u64,
    pub last_collection_objects_scanned: u64,
    pub last_collection_objects_live: u64,
    pub last_collection_objects_dead: u64,
    pub last_collection_bytes_freed: u64,
    pub last_collection_duration_us: u64,
    pub max_collection_duration_us: u64,
    pub min_collection_duration_us: u64,
    /// 最近一次 mark 播种的逐组根值计数（下标为 `RootGroup` 变体下标，和为根值总数）。
    pub root_counts: [u64; RootGroup::COUNT],
    pub(crate) mark_stack: Vec<*mut JsObject>,
    pub(crate) live_strings: HashSet<*mut JsString, FxBuildHasher>,
    pub(crate) live_bigints: HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
    pub(crate) live_cells: HashSet<*mut Cell, FxBuildHasher>,
}

impl SessionGc {
    /// 创建全零统计、空标记栈的空 GC 实例。
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionGc {
    /// 清空 session 对象表的 GC mark 位。置位不随表清位消失，残留位会让下一次
    /// `mark` 的 DFS 在已标对象处短路，漏扫其新增引用边。
    pub(crate) fn clear_all_marks(&mut self, vm: &mut Vm) {
        vm_debug!("[GC] clear_all_marks: {} session objects", vm.realm.gc.borrow().session_object_ptrs.len());
        self.clear_session_marks(vm);
    }

    /// 只清 session 对象表的 mark 位。原地 sweep 后死对象已出表、存活对象
    /// 的位在活分支清掉，正常收集路径无残留位；此臂处理 strings-only 路径
    /// 与收集前历史残留——残留 true 会让下一次 `mark` 的 DFS 在已标对象处
    /// 短路，漏扫其新增引用边。
    pub(crate) fn clear_session_marks(&mut self, vm: &mut Vm) {
        for &ptr in &vm.realm.gc.borrow().session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: session_object_ptrs 中的指针来自统一入口的 Box 化分配
            // （alloc_object_box），对象堆内指针在释放前有效。
            unsafe { (*ptr).set_gc_mark(false) };
        }
    }

    /// 只读核算 session 对象的堆数据字节（属性/元素/meta Vec capacity + upvalue 列表
    /// capacity + native 状态盒）。与收尾释放口径一致（capacity）；upvalue 列表因原件与
    /// 晋升克隆别名，释放走收尾集中去重而非逐对象路径。不释放、不置空任何指针。
    pub(crate) fn object_heap_data_bytes(obj: &JsObject) -> u64 {
        let mut bytes = 0u64;

        let elems_ptr = obj.array_elements_raw() as *const Vec<JsValue>;
        if !elems_ptr.is_null() {
            unsafe {
                bytes += size_of::<Vec<JsValue>>() as u64 + ((*elems_ptr).capacity() * size_of::<JsValue>()) as u64;
            }
        }

        let elems_meta_ptr = obj.array_elements_meta_raw() as *const Vec<Option<PropMetaEntry>>;
        if !elems_meta_ptr.is_null() {
            unsafe {
                bytes += size_of::<Vec<Option<PropMetaEntry>>>() as u64
                    + ((*elems_meta_ptr).capacity() * size_of::<Option<PropMetaEntry>>()) as u64;
            }
        }

        let hash_ptr = obj.hash_props_raw() as *const Vec<JsValue>;
        if !hash_ptr.is_null() {
            unsafe {
                bytes += size_of::<Vec<JsValue>>() as u64 + ((*hash_ptr).capacity() * size_of::<JsValue>()) as u64;
            }
        }

        let meta_ptr = obj.prop_meta_raw() as *const Vec<Option<PropMetaEntry>>;
        if !meta_ptr.is_null() {
            unsafe {
                bytes += size_of::<Vec<Option<PropMetaEntry>>>() as u64
                    + ((*meta_ptr).capacity() * size_of::<Option<PropMetaEntry>>()) as u64;
            }
        }

        let upvalues_ptr = obj.upvalues as *const Vec<*mut oxide_types::object::Cell>;
        if !upvalues_ptr.is_null() {
            unsafe {
                bytes += size_of::<Vec<*mut oxide_types::object::Cell>>() as u64
                    + ((*upvalues_ptr).capacity() * size_of::<*mut oxide_types::object::Cell>()) as u64;
            }
        }

        bytes += map::map_native_size(obj);
        bytes += set::set_native_size(obj);
        bytes += module::module_ns_native_size(obj);
        bytes += disposable_stack::disposable_stack_native_size(obj);
        bytes += array_buffer::array_buffer_native_size(obj);
        bytes += array_buffer::shared_array_buffer_native_size(obj);
        bytes += regexp::regexp_native_size(obj);
        bytes += typed_array::typed_array_native_size(obj);
        bytes += data_view::data_view_native_size(obj);
        bytes += crate::generator::generator_native_size(obj);
        bytes += crate::promise::promise_native_size(obj);
        bytes += crate::async_func::async_native_size(obj);
        bytes += crate::async_generator::async_generator_native_size(obj);
        bytes += weak_map::weak_map_native_size(obj);
        bytes += message_channel::message_port_native_size(obj);
        bytes += broadcast_channel::broadcast_channel_native_size(obj);
        bytes += event::event_native_size(obj);
        bytes += crate::arguments_gc::arguments_native_size(obj);

        bytes
    }

    /// 扫描 `obj` 全部引用边，session 对象子节点推入 `stack`，字符串边标记存活，
    /// BigInt 边标记存活。
    ///
    /// 单趟替代原 `object_edges` + `record_object_string_edges` 双遍模式：消除每对象
    /// Vec 分配与重复字段遍历。
    fn scan_edges_for_mark(
        obj: &JsObject, stack: &mut Vec<*mut JsObject>, live_strings: &mut HashSet<*mut JsString, FxBuildHasher>,
        live_bigints: &mut HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
        live_cells: &mut HashSet<*mut Cell, FxBuildHasher>,
    ) {
        if let Some(elements) = obj.array_elements_vec() {
            for &value in elements.iter() {
                Self::process_edge(value, stack, live_strings, live_bigints);
            }
        }
        if let Some(meta) = obj.array_elements_meta_vec() {
            for entry in meta.iter().flatten() {
                Self::process_edge(entry.get, stack, live_strings, live_bigints);
                Self::process_edge(entry.set, stack, live_strings, live_bigints);
            }
        }
        if let Some(props) = obj.hash_props_vec() {
            for &value in props.iter() {
                Self::process_edge(value, stack, live_strings, live_bigints);
            }
        }
        if let Some(meta) = obj.prop_meta_vec() {
            for entry in meta.iter().flatten() {
                Self::process_edge(entry.get, stack, live_strings, live_bigints);
                Self::process_edge(entry.set, stack, live_strings, live_bigints);
            }
        }
        Self::process_edge(obj.proto(), stack, live_strings, live_bigints);
        Self::process_edge(obj.captured_this(), stack, live_strings, live_bigints);
        Self::process_edge(obj.home_object(), stack, live_strings, live_bigints);
        Self::process_edge(obj.boxed_value(), stack, live_strings, live_bigints);
        // native 载荷家族边：单点分类 + 家族表函数引用驱动，
        // 替代逐族 is_* 谓词链（各家族边函数引用见 native_box_dispatch）。
        let ops = native_box_dispatch::ops_for(native_box_dispatch::classify(obj));
        if let Some(object_edges) = ops.object_edges {
            for value in object_edges(obj) {
                Self::process_edge(value, stack, live_strings, live_bigints);
            }
        }
        if let Some(string_edges) = ops.string_edges {
            for ptr in string_edges(obj) {
                Self::mark_string_live(live_strings, ptr);
            }
        }
        if let Some(cell_edges) = ops.cell_edges {
            for ptr in cell_edges(obj) {
                live_cells.insert(ptr);
            }
        }
        // 遍历 upvalue cell 中的引用：cell 指针本身入存活集（对象 upvalues 边
        // 是 cell 的根面），值边照常走 process_edge。
        for cell_ptr in obj.upvalues_slice() {
            if cell_ptr.is_null() {
                continue;
            }
            live_cells.insert(*cell_ptr);
            let cell = unsafe { &**cell_ptr };
            Self::process_edge(cell.value, stack, live_strings, live_bigints);
        }
    }

    #[inline]
    fn process_edge(
        value: JsValue, stack: &mut Vec<*mut JsObject>, live_strings: &mut HashSet<*mut JsString, FxBuildHasher>,
        live_bigints: &mut HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
    ) {
        if value.is_object() {
            let ptr = value.as_js_object_ptr();
            // perm 对象（builtin world / global 等）不回收，但可持有 session
            // 子引用（defineProperty 写入 builtin 原型等）：入栈由 DFS 扫边
            // 标活，否则该子引用永不被标活 → 原地 sweep 误释放 → 悬垂。
            if !ptr.is_null() {
                stack.push(ptr);
            }
        } else if value.is_string() {
            Self::mark_string_live(live_strings, value.as_string_ptr_mut());
        } else if value.is_bigint() {
            live_bigints.insert(value.as_bigint_ptr() as *mut num_bigint::BigInt);
        }
    }

    /// 把字符串指针标记为存活并传播 rope 闭包：Cons 迭代传播左右子节点与
    /// 扁平化产物（显式栈防深链爆栈）。perm 指针进表无害——sweep 只遍历
    /// `session_string_ptrs`。遗漏子节点 = 子节点被 sweep 释放 → 悬垂 UB，
    /// 全部字符串 insert 点必须收口此入口。
    fn mark_string_live(live: &mut HashSet<*mut JsString, FxBuildHasher>, root: *mut JsString) {
        if root.is_null() {
            return;
        }
        let mut stack = vec![root];
        while let Some(ptr) = stack.pop() {
            if ptr.is_null() || !live.insert(ptr) {
                continue;
            }
            // SAFETY: ptr 是合法 JsString 指针（session 或 perm），mark 期存活。
            let node = unsafe { &*ptr };
            if node.is_cons() {
                for child in node.cons_children() {
                    // 子节点与父同属一个 JsString 堆对象族，const 转 mut 仅用于
                    // 集合统一，不引入写操作。
                    stack.push(child as *mut JsString);
                }
                let flat = node.flat_cache_ptr();
                if !flat.is_null() {
                    stack.push(flat as *mut JsString);
                }
            }
        }
    }

    /// 释放一个 session `JsString`（`Box::into_raw` 分配），连带释放 rope
    /// 扁平化产物。**不递归子节点**——子节点独立登记主表、各自判定存活，
    /// 递归即 double-free。
    ///
    /// # Safety
    /// `ptr` 必须是 `new_string`/`new_cons_string` 中 `Box::into_raw` 产生的非空
    /// 指针，仍登记在 session 字符串表（或由 full_reset 统一清表），且恰好
    /// 释放一次。
    pub(crate) unsafe fn drop_session_string_box(ptr: *mut JsString) {
        if ptr.is_null() {
            return;
        }
        // 连带释放 rope 载荷（ConsNode 与其扁平化产物）；Flat 串无载荷。
        // SAFETY: cons_node_ptr 由 new_cons 产生，本指针恰好释放一次。
        JsString::drop_cons_node((*ptr).cons_node_ptr());
        drop(Box::from_raw(ptr));
    }

    /// 把 `obj` 直接持有的 session 字符串值记入 `live`。JsString 不持有 GC 引用，
    /// 因此"到达"一个字符串就等于标记它——不存在字符串 DFS 栈。永久字符串也会被
    /// 无害地记录；sweep 只遍历 `session_string_ptrs`，`live` 中的非 session 指针
    /// 永远不会被查询。
    #[cfg(debug_assertions)]
    fn record_object_string_edges(live: &mut HashSet<*mut JsString, FxBuildHasher>, obj: &JsObject) {
        if let Some(elements) = obj.array_elements_vec() {
            for value in elements.iter() {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if let Some(props) = obj.hash_props_vec() {
            for value in props.iter() {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.captured_this().is_string() {
            Self::mark_string_live(live, obj.captured_this().as_string_ptr_mut());
        }
        if obj.home_object().is_string() {
            Self::mark_string_live(live, obj.home_object().as_string_ptr_mut());
        }
        if obj.boxed_value().is_string() {
            Self::mark_string_live(live, obj.boxed_value().as_string_ptr_mut());
        }
        // 扫描 upvalue cell 中的字符串引用。
        for cell_ptr in obj.upvalues_slice() {
            if cell_ptr.is_null() {
                continue;
            }
            let cell = unsafe { &**cell_ptr };
            if cell.value.is_string() {
                Self::mark_string_live(live, cell.value.as_string_ptr_mut());
            }
        }
        if obj.is_map() {
            for value in map::map_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_set() {
            for value in set::set_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_weak_map_obj() {
            for value in weak_map::weak_map_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_module_namespace() {
            for value in module::module_ns_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_disposable_stack_obj() || obj.is_async_disposable_stack_obj() {
            for value in disposable_stack::dispose_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_generator_obj() {
            for ptr in crate::generator::generator_native_string_edges(obj) {
                Self::mark_string_live(live, ptr);
            }
        }
        if obj.is_promise_obj() {
            for value in crate::promise::promise_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_async_obj() {
            for ptr in crate::async_func::async_native_string_edges(obj) {
                Self::mark_string_live(live, ptr);
            }
        }
        if obj.is_async_generator_obj() {
            for ptr in crate::async_generator::async_generator_native_string_edges(obj) {
                Self::mark_string_live(live, ptr);
            }
        }
        // RegExp 实例 source/flags 字段持有字符串边。
        if obj.is_regexp_obj() {
            if obj.get_regexp_source().is_string() {
                Self::mark_string_live(live, obj.get_regexp_source().as_string_ptr_mut());
            }
            if obj.get_regexp_flags().is_string() {
                Self::mark_string_live(live, obj.get_regexp_flags().as_string_ptr_mut());
            }
        }
        // Event 载荷盒 type 字段持有字符串边。
        if obj.is_event_obj() {
            for value in event::event_native_edges(obj) {
                if value.is_string() {
                    Self::mark_string_live(live, value.as_string_ptr_mut());
                }
            }
        }
    }

    /// 从 VM roots 标记存活 session 对象、字符串与 BigInt，供 `sweep` 判定。
    ///
    /// 对象 DFS 跟踪 session 对象单表（统一入口 Box 化后无 epoch 表）；perm
    /// 对象不置位（其位跨收集残留会令 DFS 短路漏扫），走独立已访集防环。
    /// 字符串边走 rope 闭包传播（Cons 子节点与扁平化产物），BigInt 边直接入
    /// 存活集。roots 由 `Vm::for_each_root` 枚举。只置位不搬移，调用前须先
    /// `clear_all_marks` 清位。
    ///
    /// # 副作用
    /// - `root_counts` 在播种开头重置、按根组累加（和为根值总数）。
    pub(crate) fn mark(&mut self, vm: &Vm) {
        // 播种开头重置逐组计数：计数器恒反映最近一次 mark 的播种画像。
        self.root_counts = [0; RootGroup::COUNT];

        let mut seeds = Vec::new();
        let mut string_seeds: Vec<*mut JsString> = Vec::new();
        let mut bigint_seeds: Vec<*mut num_bigint::BigInt> = Vec::new();
        let counts = &mut self.root_counts;
        vm.for_each_root(|group, root| {
            counts[group as usize] += 1;
            if root.is_object() {
                seeds.push(root.as_js_object_ptr());
            } else if root.is_string() {
                string_seeds.push(root.as_string_ptr_mut());
            } else if root.is_bigint() {
                bigint_seeds.push(root.as_bigint_ptr() as *mut num_bigint::BigInt);
            }
        });

        // 根数口径是 `for_each_root` 枚举出的根值总数（24 组之和）。
        let root_total: u64 = self.root_counts.iter().sum();
        vm_debug!("[GC] mark phase: {} roots", root_total);
        vm_debug!(
            "[GC] mark phase root groups: regs={}, immutables={}, frames={}, save_stack={}, spill_stack={}, cell_stack={}, exception_value={}, pending_exception={}, last_uncaught={}, pending_length_exception={}, pending_completion={}, generator_suspended={}, delegated_iterator={}, async_context={}, async_gen_context={}, pending_async_escape={}, inline_callee={}, template_objects={}, number_to_string_cache={}, for_of_iters={}, job_queue={}, atomics_waiters={}, for_in_iters={}, global={}",
            self.root_counts[RootGroup::Regs as usize],
            self.root_counts[RootGroup::Immutables as usize],
            self.root_counts[RootGroup::Frames as usize],
            self.root_counts[RootGroup::SaveStack as usize],
            self.root_counts[RootGroup::SpillStack as usize],
            self.root_counts[RootGroup::CellStack as usize],
            self.root_counts[RootGroup::ExceptionValue as usize],
            self.root_counts[RootGroup::PendingException as usize],
            self.root_counts[RootGroup::LastUncaught as usize],
            self.root_counts[RootGroup::PendingLengthException as usize],
            self.root_counts[RootGroup::PendingCompletion as usize],
            self.root_counts[RootGroup::GeneratorSuspended as usize],
            self.root_counts[RootGroup::DelegatedIterator as usize],
            self.root_counts[RootGroup::AsyncContext as usize],
            self.root_counts[RootGroup::AsyncGenContext as usize],
            self.root_counts[RootGroup::PendingAsyncEscape as usize],
            self.root_counts[RootGroup::InlineCallee as usize],
            self.root_counts[RootGroup::TemplateObjects as usize],
            self.root_counts[RootGroup::NumberToStringCache as usize],
            self.root_counts[RootGroup::ForOfIters as usize],
            self.root_counts[RootGroup::JobQueue as usize],
            self.root_counts[RootGroup::AtomicsWaiters as usize],
            self.root_counts[RootGroup::ForInIters as usize],
            self.root_counts[RootGroup::Global as usize],
        );

        let Self {
            mark_stack: stack,
            live_strings,
            live_bigints,
            live_cells,
            ..
        } = self;
        stack.clear();
        live_strings.clear();
        live_bigints.clear();
        live_cells.clear();
        // 活跃帧 cell_stack 是 cell 的根面（不经对象图）：逐层种子入存活集。
        for cell_vec in &vm.cell_stack {
            for &cell_ptr in cell_vec {
                if !cell_ptr.is_null() {
                    live_cells.insert(cell_ptr);
                }
            }
        }
        for ptr in string_seeds {
            Self::mark_string_live(live_strings, ptr);
        }
        for ptr in bigint_seeds {
            live_bigints.insert(ptr);
        }

        // perm 对象不入 sweep 表、mark 位不被 clear_all_marks 清除，无法用 mark 位
        // 防重访；builtin 图有环（如 Object.prototype.constructor → Object），
        // 须独立已访集合，否则 DFS 无限重访。每轮 mark 重置。
        let mut visited_perm = HashSet::with_hasher(FxBuildHasher);
        for ptr in seeds {
            if ptr.is_null() {
                continue;
            }
            if vm.is_session_ptr(ptr) {
                stack.push(ptr);
                continue;
            }
            // P 对象根（builtin world / global）不回收，只扫边；登记已访防
            // 后续作为 perm 边被重扫。
            // SAFETY: 对象根由 VM 自有的字段与 builtin 对象产生，指针合法。
            unsafe {
                let obj = &*ptr;
                if visited_perm.insert(ptr) {
                    Self::scan_edges_for_mark(obj, stack, live_strings, live_bigints, live_cells);
                }
            }
        }

        while let Some(ptr) = stack.pop() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 由根/session 边发现，session 根检查保证它是合法对象指针。
            unsafe {
                let obj = &mut *ptr;
                // perm 对象不回收：只扫边标活 session 子引用，不置 mark 位（其位
                // 跨收集残留会令下次 DFS 短路漏扫）。已访则跳过，防 perm 环重入。
                if !vm.is_session_ptr(ptr) {
                    if visited_perm.insert(ptr) {
                        Self::scan_edges_for_mark(obj, stack, live_strings, live_bigints, live_cells);
                    }
                    continue;
                }
                if obj.is_gc_marked() {
                    continue;
                }
                obj.set_gc_mark(true);
                Self::scan_edges_for_mark(obj, stack, live_strings, live_bigints, live_cells);
            }
        }
    }

    /// 释放对象本体之外的堆外属性数据（元素区、元素 meta、hash 属性区、属性 meta
    /// 与各族 native 状态盒），按 capacity 经 `Box::from_raw` 各恰好释放一次，返回
    /// 字节数。`obj_ptr` 为空时返回 0。不含 upvalue 列表与对象本体，由调用方处理。
    ///
    /// # 边界与前提
    /// - 统一对象表内全部对象为 session 对象（统一入口 Box 化后生产路径无
    ///   epoch 原件），本函数只服务表内对象的释放路径，不做归属断言。
    pub(crate) fn drop_object_heap_data(obj_ptr: *mut JsObject) -> u64 {
        if obj_ptr.is_null() {
            return 0;
        }
        // SAFETY: `obj_ptr` 在调用本辅助函数前已校验，指向 VM 拥有的 session 对象。
        // 只重建 JsObject::ensure_hash_props/ensure_prop_meta 分配的 Box，
        // 且只在这里释放一次。
        unsafe {
            let obj = &mut *obj_ptr;
            let mut freed_bytes = 0u64;

            let elems_ptr = obj.array_elements_raw() as *mut Vec<JsValue>;
            if !elems_ptr.is_null() {
                let vec = Box::from_raw(elems_ptr);
                freed_bytes += size_of::<Vec<JsValue>>() as u64 + (vec.capacity() * size_of::<JsValue>()) as u64;
                std::mem::drop(vec);
            }

            let elems_meta_ptr = obj.array_elements_meta_raw() as *mut Vec<Option<PropMetaEntry>>;
            if !elems_meta_ptr.is_null() {
                let vec = Box::from_raw(elems_meta_ptr);
                freed_bytes += size_of::<Vec<Option<PropMetaEntry>>>() as u64
                    + (vec.capacity() * size_of::<Option<PropMetaEntry>>()) as u64;
                std::mem::drop(vec);
            }

            let hash_ptr = obj.hash_props_raw() as *mut Vec<JsValue>;
            if !hash_ptr.is_null() {
                let vec = Box::from_raw(hash_ptr);
                freed_bytes += size_of::<Vec<JsValue>>() as u64 + (vec.capacity() * size_of::<JsValue>()) as u64;
                std::mem::drop(vec);
            }

            let meta_ptr = obj.prop_meta_raw() as *mut Vec<Option<PropMetaEntry>>;
            if !meta_ptr.is_null() {
                let vec = Box::from_raw(meta_ptr);
                freed_bytes += size_of::<Vec<Option<PropMetaEntry>>>() as u64
                    + (vec.capacity() * size_of::<Option<PropMetaEntry>>()) as u64;
                std::mem::drop(vec);
            }

            freed_bytes += map::drop_map_native(obj);
            freed_bytes += set::drop_set_native(obj);
            freed_bytes += module::drop_module_ns_native(obj);
            freed_bytes += disposable_stack::drop_dispose_native(obj);
            freed_bytes += array_buffer::drop_array_buffer_native(obj);
            freed_bytes += array_buffer::drop_shared_array_buffer_native(obj);
            freed_bytes += regexp::drop_regexp_native(obj);
            freed_bytes += typed_array::drop_typed_array_native(obj);
            freed_bytes += data_view::drop_data_view_native(obj);
            freed_bytes += crate::generator::drop_generator_native(obj);
            freed_bytes += crate::promise::drop_promise_native(obj);
            freed_bytes += crate::async_func::drop_async_native(obj);
            freed_bytes += crate::async_generator::drop_async_generator_native(obj);
            freed_bytes += weak_map::drop_weak_map_native(obj);
            freed_bytes += message_channel::drop_message_port_native(obj);
            freed_bytes += broadcast_channel::drop_broadcast_channel_native(obj);
            freed_bytes += event::drop_event_native(obj);
            freed_bytes += crate::arguments_gc::drop_arguments_native(obj);

            freed_bytes
        }
    }

    /// 释放一个死 session 对象：对象本体 + 堆数据 + upvalue 列表，返回释放字节数。
    ///
    /// # 步骤
    /// 1. 释放堆数据（元素区 / 属性区 / meta 区 / native 状态盒）。
    /// 2. 释放 upvalue 列表 Box 并置空对象侧字段。
    /// 3. 释放对象本体：堆载体（HEAP_BIT）经 `Box::from_raw` 释放；
    ///    Bump 载体（晋升族克隆，仅测试形态）随旧 Bump 换新归还，不在此释放。
    ///
    /// # 边界与前提
    /// - `obj_ptr` 必须非空：空指针时第 1 步返回 0，但本体字节与位域读取
    ///   对空指针无效，调用点（sweep 死分支 / 收尾逐对象路径）先行跳过空位。
    /// - 仅 sweep 死分支与统一收尾到达此处：死对象从不被克隆，upvalue 列表
    ///   Box 无其他持有者，在此恰好释放一次；存活分支绝不释放（若有克隆仍
    ///   引用同一 Box）。释放后死对象已移出对象表，收尾统一释放按表枚举
    ///   不会再见，置空保证对象侧幂等、无陈旧指针。
    pub(crate) fn drop_dead_session_object(obj_ptr: *mut JsObject) -> u64 {
        let mut freed = Self::drop_object_heap_data(obj_ptr) + size_of::<JsObject>() as u64;
        // SAFETY: obj_ptr 来自 session 对象表，sweep 期间仍指向合法对象；
        // upvalues Box 仅本对象持有（死对象不克隆），保证恰好释放一次。
        unsafe {
            let up = (*obj_ptr).upvalues;
            if !up.is_null() {
                let vec = Box::from_raw(up as *mut Vec<*mut oxide_types::object::Cell>);
                freed += size_of::<Vec<*mut oxide_types::object::Cell>>() as u64
                    + (vec.capacity() * size_of::<*mut oxide_types::object::Cell>()) as u64;
                (*obj_ptr).upvalues = std::ptr::null_mut();
                std::mem::drop(vec);
            }
            // 释放对象本体：堆载体（HEAP_BIT）经 Box::from_raw 释放，Bump
            // 载体（克隆）随旧 Bump 换新归还，不在此释放。
            if (*obj_ptr).is_heap_alloc() {
                drop(Box::from_raw(obj_ptr));
            }
        }
        freed
    }

    /// 释放一个已死 session `JsString`（由 `Vm::new_string`/`new_cons_string`
    /// 经 `Box::into_raw` 分配），返回释放的字节数（含 rope 扁平化产物）。
    /// 与 `Vm::free_session_string_heap_data` 的释放一致，但只选择性作用于
    /// 单个已死指针。
    ///
    /// # Safety
    /// `ptr` 必须是 `Box::into_raw(Box::new(JsString))` 产生的非空指针，仍存在
    /// 于 `session_string_ptrs`，且恰好释放一次。
    unsafe fn drop_dead_session_string(ptr: *mut JsString) -> u64 {
        let bytes = (size_of::<JsString>() + (*ptr).payload_bytes()) as u64;
        Self::drop_session_string_box(ptr);
        bytes
    }

    /// 原地清扫 session 对象：存活对象带位保留（清位并入活分支）、地址不变，
    /// 死对象释放本体、堆区与独占 upvalue 列表并出表；弱表弱键在死对象释放前
    /// 按 mark 位定生死（死键条目丢弃）。免转发表与根重写。
    ///
    /// # 步骤
    /// 1. 弱键定夺：对每个存活弱表按 mark 位重建条目表（此刻全部键对象
    ///    仍分配，位域可读，见 `resolve_weak_key_sweep` 时序前提）。
    /// 2. 分流：已标对象保留并清位（无残留位），未标对象原地释放出表。
    /// 3. 表回写存活集，按存活重算对象口径账目，累计扫描/存活/死统计。
    ///
    /// # 副作用
    /// - `session_object_ptrs` 仅剩存活对象；`session_bytes_allocated` 重算为
    ///   存活对象字节（串分量由串清扫补回；BigInt/cell 不在手工账目内，
    ///   由 `run_alloc_bytes` 公式按表长单列）；
    /// - `total_*` 与 `last_collection_*` 统计按对象口径更新（绝对赋值，
    ///   串/BigInt/cell 清扫在其上累加）。
    ///
    /// # 边界与前提
    /// - 调用前须完成 `mark`（位域为本次收集的判定依据）；
    /// - 死对象从不被克隆，upvalue 列表 Box 无其他持有者，恰好一次释放成立。
    pub(crate) fn sweep(&mut self, vm: &mut Vm) -> u64 {
        self.sweep_in_place(vm).2
    }

    /// 原地 sweep 核心：弱键定夺、死对象释放与出表、存活重算、统计累计。
    /// `sweep`（完整收集）与 `collect_in_run`（执行期收集）共用，保证两条
    /// 收集路径的弱键定夺口径一致。返回 (存活数, 死数, 释放字节)。
    fn sweep_in_place(&mut self, vm: &mut Vm) -> (u64, u64, u64) {
        let old_ptrs = std::mem::take(&mut vm.realm.gc.borrow_mut().session_object_ptrs);
        let mut survivors = Vec::with_capacity(old_ptrs.len());

        // 弱键定夺先于死对象释放：键对象（含死键）此刻全部仍分配，
        // 对象头位域读取安全（时序前提见 resolve_weak_key_sweep）。
        for &ptr in &old_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session 对象表登记，sweep 运行期间有效。
            let obj = unsafe { &*ptr };
            if obj.is_gc_marked() && obj.is_weak_map_obj() {
                // SAFETY: 弱表 native 盒独占，整表重建在定夺期间无并发读者。
                unsafe {
                    weak_map::rewrite_weak_map_native(&mut *ptr, resolve_weak_key_sweep, |value| value);
                }
            }
        }

        // 注册表剪枝：按 mark 位移除死通道对象（此刻全部对象仍分配、位域可读，
        // 与弱键定夺同时序）。死通道对象在随后的分流循环被释放，注册表已先移除
        // 其条目，无悬垂；空列表删键。
        for channels in vm.realm.gc.borrow_mut().broadcast_channels.values_mut() {
            channels.retain(|&ptr| {
                if ptr.is_null() {
                    return false;
                }
                // SAFETY: ptr 来自 session 对象表登记，sweep 运行期间有效。
                unsafe { (*ptr).is_gc_marked() }
            });
        }
        vm.realm
            .gc
            .borrow_mut()
            .broadcast_channels
            .retain(|_, channels| !channels.is_empty());

        // EventTarget 监听器注册表剪枝：按 mark 位移除死目标对象（弱键，同
        // broadcast_channels 时序——死目标在随后的分流循环被释放，注册表已先
        // 移除其条目，无悬垂）。
        vm.realm
            .gc
            .borrow_mut()
            .event_targets
            .retain(|&ptr, _| {
                if ptr.is_null() {
                    return false;
                }
                // SAFETY: ptr 来自 session 对象表登记，sweep 运行期间有效。
                unsafe { (*ptr).is_gc_marked() }
            });

        // 按 mark 位分流：存活保留清位，死对象释放本体、堆区与 upvalue 出表。
        let mut dead = 0u64;
        let mut freed_bytes = 0u64;
        for &ptr in &old_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session 对象表登记，sweep 运行期间有效。
            if unsafe { (*ptr).is_gc_marked() } {
                // 原地 sweep 不搬移对象：存活对象带位保留，清位并入活分支
                // （收集后无残留，残留 true 使下一次 mark DFS 短路漏标）。
                unsafe { (*ptr).set_gc_mark(false) };
                survivors.push(ptr);
            } else {
                // SAFETY: 死对象从不被克隆，upvalue 列表 Box 无其他持有者。
                freed_bytes += Self::drop_dead_session_object(ptr);
                dead += 1;
            }
        }
        let live = survivors.len() as u64;
        vm.realm.gc.borrow_mut().session_object_ptrs = survivors;

        // 对象口径重算：存活对象头 + 堆数据求和（串/BigInt/cell 分量不在此口径，
        // 由各清扫路径按存活补回，与完整收集的最终账目一致）。读入局部变量后
        // 释放借用再写回，不跨读写持借。
        let new_bytes: usize = vm
            .realm
            .gc
            .borrow()
            .session_object_ptrs
            .iter()
            .filter(|&&ptr| !ptr.is_null())
            .map(|&ptr| {
                // SAFETY: 存活对象在 session 表中，sweep 运行期间有效。
                let obj = unsafe { &*ptr };
                size_of::<JsObject>() as u64 + Self::object_heap_data_bytes(obj)
            })
            .sum::<u64>() as usize;
        vm.realm.gc.borrow_mut().session_bytes_allocated = new_bytes;

        // 统计按对象口径累计（串/BigInt/cell 清扫在其上累加释放字节）。
        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed_bytes);
        self.total_objects_scanned += live + dead;
        self.total_objects_live += live;
        self.total_objects_dead += dead;
        self.last_collection_objects_scanned = live + dead;
        self.last_collection_objects_live = live;
        self.last_collection_objects_dead = dead;
        self.last_collection_bytes_freed = freed_bytes;

        if live + dead > 0 {
            if dead == 0 {
                vm_debug!("[GC] sweep phase -> no objects collected ({} live, {} dead)", live, dead);
            } else {
                vm_debug!(
                    "[GC] sweep phase: {} scanned, {} live, {} dead, {} bytes",
                    live + dead,
                    live,
                    dead,
                    freed_bytes
                );
            }
        }
        (live, dead, freed_bytes)
    }

    /// 清扫 session `JsString`：保留 `mark()` 阶段记为存活的部分，其余经
    /// `Box::from_raw` 释放。存活字符串不被搬移——Box 地址稳定——因此无需
    /// forwarding 表与根指针重写。在对象清扫之后运行（对象清扫会把
    /// `session_bytes_allocated` 重置为仅对象），再补回存活字符串字节。
    /// 返回释放的字节数。
    pub(crate) fn sweep_session_strings(&mut self, vm: &mut Vm) -> u64 {
        vm_debug!("[GC] sweep strings: {} string ptrs", vm.realm.gc.borrow().session_string_ptrs.len());
        let old = std::mem::take(&mut vm.realm.gc.borrow_mut().session_string_ptrs);
        let mut freed = 0u64;
        let mut live_bytes = 0usize;
        let mut live = Vec::with_capacity(old.len());
        for ptr in old {
            if ptr.is_null() {
                continue;
            }
            if self.live_strings.contains(&ptr) {
                // 存活——地址不变，无需重写。
                // SAFETY: ptr 是仍归 VM 所有的存活 session 字符串 box。
                live_bytes += unsafe { size_of::<JsString>() + (*ptr).payload_bytes() };
                live.push(ptr);
            } else {
                // SAFETY: ptr 在 session_string_ptrs 中但不可达，恰好释放一次。
                freed += unsafe { Self::drop_dead_session_string(ptr) };
            }
        }
        vm.realm.gc.borrow_mut().session_string_ptrs = live;
        let new_bytes = vm.realm.gc.borrow().session_bytes_allocated.saturating_add(live_bytes);
        vm.realm.gc.borrow_mut().session_bytes_allocated = new_bytes;

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    /// 清扫 session `BigInt`：保留 `mark()` 阶段记为存活的部分，其余经
    /// `Box::from_raw` 释放。存活 BigInt 不被搬移——Box 地址稳定——因此无需
    /// forwarding 表与根指针重写。在字符串清扫之后运行。
    /// 返回释放的字节数。
    pub(crate) fn sweep_session_bigints(&mut self, vm: &mut Vm) -> u64 {
        let old = vm
            .realm
            .gc
            .borrow_mut()
            .session_bigint_ptrs
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        let mut freed = 0u64;
        let mut live = Vec::with_capacity(old.len());
        for ptr in old {
            if ptr.is_null() {
                continue;
            }
            if self.live_bigints.contains(&ptr) {
                // 存活——地址不变，无需重写。BigInt 不入手工账目，
                // 存活字节由 `run_alloc_bytes` 公式按表长单列。
                live.push(ptr);
            } else {
                // SAFETY: ptr 在 session_bigint_ptrs 中但不可达，恰好释放一次。
                freed += size_of::<num_bigint::BigInt>() as u64;
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
        *vm.realm.gc.borrow_mut().session_bigint_ptrs.borrow_mut() = live;

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    /// 清扫 session upvalue cell：保留 `mark()` 阶段记为存活的部分，其余经
    /// `Box::from_raw` 释放。存活 cell 不被搬移——Box 地址稳定——因此无需
    /// forwarding 表与根指针重写。在 BigInt 清扫之后运行。
    ///
    /// cell 不在 `session_bytes_allocated` 手工账目口径内（串分量在账目内，
    /// BigInt/cell 由 `run_alloc_bytes` 公式按表长单列），故只累计释放字节
    /// 统计、不重算存活账目。
    /// 返回释放的字节数。
    pub(crate) fn sweep_session_cells(&mut self, vm: &mut Vm) -> u64 {
        let old = vm
            .realm
            .gc
            .borrow_mut()
            .session_cell_ptrs
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        let mut freed = 0u64;
        let mut live = Vec::with_capacity(old.len());
        for ptr in old {
            if ptr.is_null() {
                continue;
            }
            if self.live_cells.contains(&ptr) {
                // 存活——地址不变，无需重写。
                live.push(ptr);
            } else {
                // SAFETY: ptr 在 session_cell_ptrs 中但不可达，恰好释放一次。
                freed += size_of::<Cell>() as u64;
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
        *vm.realm.gc.borrow_mut().session_cell_ptrs.borrow_mut() = live;

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    /// 完整收集的触发判断：对象、字符串或 BigInt 表非空，且字节账目已达阈值。
    pub(crate) fn should_collect(&self, vm: &Vm) -> bool {
        let gc = vm.realm.gc.borrow();
        (!gc.session_object_ptrs.is_empty()
            || !gc.session_string_ptrs.is_empty()
            || !gc.session_bigint_ptrs.borrow().is_empty())
            && gc.session_bytes_allocated >= vm.kernel_core().config().session_gc_threshold
    }

    /// 执行期字符串回收的触发判断：账目须超过水位（上次收集后的存活字节 +
    /// 阈值增量）。完整收集（reset）仍用 [`Self::should_collect`] 的阈值直接比较，
    /// 保证死对象超阈值即被 reset 回收；执行期走增量水位，活串超阈值时不每指令
    /// 重复触发无死串可回收的白跑。
    pub(crate) fn should_collect_strings(&self, vm: &Vm) -> bool {
        let gc = vm.realm.gc.borrow();
        (!gc.session_object_ptrs.is_empty()
            || !gc.session_string_ptrs.is_empty()
            || !gc.session_bigint_ptrs.borrow().is_empty())
            && gc.session_bytes_allocated >= gc.string_gc_watermark
    }

    /// debug 兜底：mark 之后断言所有存活对象持有的字符串边都已登记进
    /// `live_strings`——捕获"对象边漏登记 → 存活串被误释放"的静默悬垂。
    #[cfg(debug_assertions)]
    fn debug_assert_marked_object_strings_live(&self, vm: &Vm) {
        let mut live = HashSet::with_hasher(FxBuildHasher);
        for &ptr in &vm.realm.gc.borrow().session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: session_object_ptrs 归 VM 所有，指针在 arena 存活期有效。
            let obj = unsafe { &*ptr };
            if !obj.is_gc_marked() {
                continue;
            }
            live.clear();
            Self::record_object_string_edges(&mut live, obj);
            for s_ptr in live.drain() {
                assert!(self.live_strings.contains(&s_ptr), "存活对象持有未登记的 session 字符串边");
            }
        }
    }

    /// 完整收集编排：`mark` → `sweep`（对象原地清扫）→ `sweep_session_strings` →
    /// `sweep_session_bigints` → `sweep_session_cells`，累加三类释放字节；
    /// 对象与字符串、BigInt 地址均稳定、无需 forwarding。更新各表、字节账目与
    /// mark 位，累计收集次数与最近/最大/最小耗时，释放字节大于 0 或每满 100 次
    /// 时输出统计摘要。
    pub(crate) fn collect(&mut self, vm: &mut Vm) {
        let start = Instant::now();

        vm_info!("[GC] cycle #{} start", self.total_collections + 1);

        self.mark(vm);
        let mut freed_bytes = self.sweep(vm);
        freed_bytes += self.sweep_session_strings(vm);
        freed_bytes += self.sweep_session_bigints(vm);
        freed_bytes += self.sweep_session_cells(vm);

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);

        vm_info!(
            "[GC] cycle #{} end: {} scanned, {} live, {} dead, {:.1}ms, {} bytes freed",
            self.total_collections,
            self.last_collection_objects_scanned,
            self.last_collection_objects_live,
            self.last_collection_objects_dead,
            elapsed.as_secs_f64() * 1000.0,
            freed_bytes,
        );

        if freed_bytes > 0 || self.total_collections % 100 == 0 {
            vm_debug!("{}", self.stats_summary());
        }
    }

    /// 仅回收 session 字符串：完整 mark（对象只置位不搬移）→ 按存活集清扫字符串。
    ///
    /// 刻意跳过对象清扫——轻量档只走串表释放，对象口径的回收归完整收集与
    /// 执行期收集（dispatch 安全点）。字符串每串独立 Box、地址稳定，清扫
    /// 无需 forwarding 与根重写。
    ///
    /// # 副作用
    /// - 清空全部对象 mark 位；按存活集释放死串；累计 `total_collections` 与时长统计；
    ///   抬高 `string_gc_watermark`（存活字节 + 阈值增量）供下次执行期触发。
    ///
    /// # 注意事项
    /// - mark 的 DFS 以 `is_gc_marked` 短路，残留 true 会导致后续完整收集漏标，
    ///   本路径不跑对象 sweep（sweep 内清位不会执行），故开头与结尾都清位——
    ///   维持"mark 之后必由清位收尾"的不变量，strings-only 结束不留残留。
    /// - 不搬移对象，`session_bytes_allocated` 保持对象账目，手动扣掉字符串账目后
    ///   由 `sweep_session_strings` 补回存活串字节（BigInt/cell 不在手工账目内），
    ///   与完整收集的最终账目一致。
    pub(crate) fn collect_strings_only(&mut self, vm: &mut Vm) {
        let start = Instant::now();

        vm_info!("[GC] strings-only cycle #{} start", self.total_collections + 1);

        // 清残留 mark 位：防止本次字符串 mark 因历史 true 短路而漏标。
        self.clear_all_marks(vm);

        // 完整 mark：复用现有实现，产出 live_strings；对象仅置位、不搬移。
        self.mark(vm);

        // debug 兜底：存活对象的字符串边必须已登记，否则串清扫将释放活串。
        // cfg 守卫与定义一致：release 下断言体剥离，调用点同步剥离。
        #[cfg(debug_assertions)]
        self.debug_assert_marked_object_strings_live(vm);

        // 扣减字符串账目（保留对象账目），再补回存活串字节。
        let object_bytes: usize = vm
            .realm
            .gc
            .borrow()
            .session_object_ptrs
            .iter()
            .filter(|&&ptr| !ptr.is_null())
            .map(|&ptr| {
                let obj = unsafe { &*ptr };
                size_of::<JsObject>() as u64 + Self::object_heap_data_bytes(obj)
            })
            .sum::<u64>() as usize;
        vm.realm.gc.borrow_mut().session_bytes_allocated = object_bytes;
        let mut freed_bytes = self.sweep_session_strings(vm);
        freed_bytes += self.sweep_session_bigints(vm);
        freed_bytes += self.sweep_session_cells(vm);

        // 恢复 mark 位不变量：mark 之后必由清位收尾（与 sweep 末尾同点清位）。
        // 残留 marked 对象会让下一次完整收集（reset 路径）的 mark DFS 短路漏标，
        // 其字符串边不进 live_strings → 存活串被误释放 → 存活对象持悬垂指针。
        self.clear_all_marks(vm);

        // 抬高下次触发水位：本次存活字节 + 阈值增量，避免活串超阈值时每指令重复
        // 触发无死串可回收的完整 mark + 串清扫。读入局部变量后释放借用再写回。
        let threshold = vm.kernel_core().config().session_gc_threshold;
        let new_watermark = vm.realm.gc.borrow().session_bytes_allocated.saturating_add(threshold);
        vm.realm.gc.borrow_mut().string_gc_watermark = new_watermark;

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);

        vm_info!(
            "[GC] strings-only cycle #{} end: {} bytes freed, {:.1}ms",
            self.total_collections,
            freed_bytes,
            elapsed.as_secs_f64() * 1000.0,
        );
    }

    /// 执行期字符串回收触发入口：`should_collect_strings` 命中时执行 strings-only 收集。
    pub(crate) fn maybe_collect_strings_only(&mut self, vm: &mut Vm) {
        if self.should_collect_strings(vm) {
            self.collect_strings_only(vm);
        }
    }

    /// 完整收集触发入口：`should_collect` 命中时执行完整收集。
    pub(crate) fn maybe_collect(&mut self, vm: &mut Vm) {
        if self.should_collect(vm) {
            self.collect(vm);
        }
    }

    /// 执行期收集：session 原地 sweep（与完整收集共用清扫核心）。
    ///
    /// 在 dispatch 安全点（循环顶 + `native_call_depth == 0`）回收单 run 分配
    /// 包络内的死对象：死 session 对象原地释放（堆区 + upvalue 列表 + 本体）
    /// 并出表，弱键按 mark 位定夺；存活对象不移动——地址不变、免转发表与
    /// 根重写，用户可观察 identity 不分裂。
    ///
    /// # 边界与前提
    /// - 无门控：for-in 迭代器体是堆上 Box，活跃形经 VM 表、挂起形经状态盒
    ///   边进入根收集，键引用被 mark 标活，原地清扫不误释放；
    /// - 调用点为 dispatch 安全点（`native_call_depth == 0`），无在途 builtin
    ///   局部裸指针、dispatch 未重入。
    ///
    /// # 副作用
    /// - `session_object_ptrs` 仅剩存活，`session_bytes_allocated` 按存活整体
    ///   重算（对象 + 串；本路径不扫串，串表整体计入，串 GC 归 strings-only；
    ///   BigInt/cell 不在手工账目内，由公式按表长单列）；
    /// - `gc_watermark` = 当前分配包络 + 阈值增量；
    /// - 累计/时长统计更新；mark 位全清（收集后无残留）。
    pub(crate) fn collect_in_run(&mut self, vm: &mut Vm) {
        let start = Instant::now();
        vm_info!("[GC] in-run cycle #{} start", self.total_collections + 1);

        // 清残留 mark 位：防 mark DFS 因历史 true 短路漏标。
        self.clear_all_marks(vm);

        self.mark(vm);

        // 死 session 对象原地释放（堆区 + upvalue 列表 + 本体）并出表，弱键
        // 按 mark 位定夺；存活对象不动——地址不变，免转发表与根重写。对象
        // 口径统计由清扫核心累计，此处只补集合与账目。
        self.sweep_in_place(vm);

        // 死 cell 随死对象出表：存活 cell 经 mark 种子 + 对象边入存活集，此处清扫。
        self.sweep_session_cells(vm);

        // 整体口径重算：对象分量已由清扫核心写入，此处补串分量（本路径不扫
        // 串，串表整体计入，与 strings-only 口径一致；BigInt/cell 不在手工
        // 账目内，由 `run_alloc_bytes` 公式按表长单列）。
        let mut string_bytes: usize = 0;
        for &ptr in &vm.realm.gc.borrow().session_string_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 在字符串表登记，收尾前有效。
            string_bytes += size_of::<JsString>() + unsafe { (*ptr).payload_bytes() };
        }
        let new_bytes = vm.realm.gc.borrow().session_bytes_allocated.saturating_add(string_bytes);
        vm.realm.gc.borrow_mut().session_bytes_allocated = new_bytes;

        // 抬高下次触发水位：当前分配包络 + 阈值增量——存活包络超阈值时不每指令
        // 重复触发无死对象可回收的白跑。读入局部变量后释放借用再写回。
        let alloc = vm.run_alloc_bytes();
        let threshold = vm.realm.gc.borrow().gc_threshold_cached;
        vm.realm.gc.borrow_mut().gc_watermark = alloc.saturating_add(threshold);

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);

        vm_info!(
            "[GC] in-run cycle #{} end: {} scanned, {} live, {} dead, {:.1}ms, {} bytes freed",
            self.total_collections,
            self.last_collection_objects_scanned,
            self.last_collection_objects_live,
            self.last_collection_objects_dead,
            elapsed.as_secs_f64() * 1000.0,
            self.last_collection_bytes_freed,
        );
    }

    /// 单行 GC 统计摘要：收集次数、扫描/存活/死对象数、释放字节与最近耗时。
    pub(crate) fn stats_summary(&self) -> String {
        format!(
            "[GC] collection #{}: {} scanned, {} live, {} dead, {} freed, {}μs",
            self.total_collections,
            self.last_collection_objects_scanned,
            self.last_collection_objects_live,
            self.last_collection_objects_dead,
            self.last_collection_bytes_freed,
            self.last_collection_duration_us
        )
    }
}

impl Default for SessionGc {
    fn default() -> Self {
        Self {
            total_collections: 0,
            total_bytes_freed: 0,
            total_objects_scanned: 0,
            total_objects_live: 0,
            total_objects_dead: 0,
            last_collection_objects_scanned: 0,
            last_collection_objects_live: 0,
            last_collection_objects_dead: 0,
            last_collection_bytes_freed: 0,
            last_collection_duration_us: 0,
            max_collection_duration_us: 0,
            min_collection_duration_us: u64::MAX,
            root_counts: [0; RootGroup::COUNT],
            mark_stack: Vec::new(),
            live_strings: HashSet::default(),
            live_bigints: HashSet::default(),
            live_cells: HashSet::default(),
        }
    }
}

/// 原地 sweep 的弱键判定：session 键按 mark 位定生死（已标 = 强可达保留，
/// 未标 = 死键丢弃），非 session 键（perm 对象等）不可死、恒保留；symbol
/// 键按值恒等、恒保留。
///
/// # 边界与前提
/// - 时序前提：定夺须先于死对象本体释放执行——判定要读键对象本体的 mark
///   位域，调用点（sweep 弱键定夺相）位于死对象释放相之前，全部键对象（含
///   死键）此刻仍分配，位域读取安全；死键指针此后只作被丢弃的表项内容，
///   不再被解引用。
/// - 死键指针只读对象头位域，不解引用其堆数据（属性区等随释放销毁）。
pub(crate) fn resolve_weak_key_sweep(key: weak_map::WeakKey) -> Option<weak_map::WeakKey> {
    let weak_map::WeakKey::Obj(ptr) = key else {
        return Some(key);
    };
    let mut_ptr = ptr as *mut JsObject;
    // SAFETY: 见时序前提——定夺相键对象仍分配，对象头位域可读。
    let old = unsafe { &*mut_ptr };
    if old.is_session_epoch() && !old.is_gc_marked() {
        return None;
    }
    Some(key)
}

#[cfg(test)]
mod tests;
