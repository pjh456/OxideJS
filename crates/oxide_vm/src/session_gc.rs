use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::time::Instant;

use crate::{vm_debug, vm_info};
use oxide_types::object::{JsObject, JsString, PropMetaEntry};
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::vm::Vm;
use oxide_builtins::{array_buffer, data_view, disposable_stack, map, module, regexp, set, typed_array, weak_map};

/// session 级 mark-sweep GC 的状态与统计。
///
/// 回收 session arena 中不再可达的对象与 `Vm::new_string` 分配的 session 字符串：
/// `mark` 从 VM roots 标记存活对象，`sweep` 将存活对象复制进新 arena（移动式）、
/// 更新所有引用，并释放死对象；`sweep_session_strings` 按存活标记回收字符串。
/// 所有统计字段供外部观测 GC 行为。
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
    pub(crate) mark_stack: Vec<*mut JsObject>,
    pub(crate) live_strings: HashSet<*mut JsString, FxBuildHasher>,
    pub(crate) live_bigints: HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
}

impl SessionGc {
    /// 创建全零统计、空标记栈的空 GC 实例。
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionGc {
    /// 清空 session 与 epoch 全部对象的 GC mark 位。epoch 臂须与 session 同清：
    /// 置位不随 session 清位消失，残留位会让下一次 `mark` 的 DFS 在已标对象处
    /// 短路，漏扫其新增引用边。
    pub(crate) fn clear_all_marks(&mut self, vm: &mut Vm) {
        vm_debug!("[GC] clear_all_marks: {} session objects", vm.gc_state.session_object_ptrs.len());
        for &ptr in &vm.gc_state.session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: session_object_ptrs 中的指针只来自 promote_object_inner 的
            // session_epoch.alloc，在 arena 存活期间有效。
            unsafe { (*ptr).set_gc_mark(false) };
        }
        // epoch 臂同清：mark 的 epoch 臂置位不随 session 清位消除，残留位会让
        // 下一次 mark 的 DFS 短路、漏扫该对象此间新增的边。
        for &ptr in &vm.gc_state.epoch_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: epoch_object_ptrs 中的指针只来自 alloc_object 的 epoch.alloc，
            // 在 epoch 存活期间有效。
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

        bytes
    }

    /// 扫描 `obj` 全部引用边，session 对象子节点推入 `stack`，字符串边标记存活，
    /// BigInt 边标记存活。
    ///
    /// 单趟替代原 `object_edges` + `record_object_string_edges` 双遍模式：消除每对象
    /// Vec 分配与重复字段遍历。
    fn scan_edges_for_mark(
        obj: &JsObject, vm: &Vm, stack: &mut Vec<*mut JsObject>,
        live_strings: &mut HashSet<*mut JsString, FxBuildHasher>,
        live_bigints: &mut HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
    ) {
        if let Some(elements) = obj.array_elements_vec() {
            for &value in elements.iter() {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if let Some(meta) = obj.array_elements_meta_vec() {
            for entry in meta.iter().flatten() {
                Self::process_edge(entry.get, vm, stack, live_strings, live_bigints);
                Self::process_edge(entry.set, vm, stack, live_strings, live_bigints);
            }
        }
        if let Some(props) = obj.hash_props_vec() {
            for &value in props.iter() {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if let Some(meta) = obj.prop_meta_vec() {
            for entry in meta.iter().flatten() {
                Self::process_edge(entry.get, vm, stack, live_strings, live_bigints);
                Self::process_edge(entry.set, vm, stack, live_strings, live_bigints);
            }
        }
        Self::process_edge(obj.proto(), vm, stack, live_strings, live_bigints);
        Self::process_edge(obj.captured_this(), vm, stack, live_strings, live_bigints);
        Self::process_edge(obj.home_object(), vm, stack, live_strings, live_bigints);
        Self::process_edge(obj.boxed_value(), vm, stack, live_strings, live_bigints);
        if obj.is_map() {
            for value in map::map_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_set() {
            for value in set::set_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_weak_map_obj() {
            // 仅值边进 mark（强边）；键为弱边，不入栈不置位。
            for value in weak_map::weak_map_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_disposable_stack_obj() || obj.is_async_disposable_stack_obj() {
            for value in disposable_stack::dispose_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_typed_array_obj() {
            for value in typed_array::typed_array_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_data_view_obj() {
            for value in data_view::data_view_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_module_namespace() {
            for value in module::module_ns_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_generator_obj() {
            for value in crate::generator::generator_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
            for ptr in crate::generator::generator_native_string_edges(obj) {
                Self::mark_string_live(live_strings, ptr);
            }
        }
        if obj.is_promise_obj() {
            for value in crate::promise::promise_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
        }
        if obj.is_async_obj() {
            for value in crate::async_func::async_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
            for ptr in crate::async_func::async_native_string_edges(obj) {
                Self::mark_string_live(live_strings, ptr);
            }
        }
        if obj.is_async_generator_obj() {
            for value in crate::async_generator::async_generator_native_edges(obj) {
                Self::process_edge(value, vm, stack, live_strings, live_bigints);
            }
            for ptr in crate::async_generator::async_generator_native_string_edges(obj) {
                Self::mark_string_live(live_strings, ptr);
            }
        }
        // RegExp 实例 source/flags 字段持有字符串边。
        if obj.is_regexp_obj() {
            Self::process_edge(obj.get_regexp_source(), vm, stack, live_strings, live_bigints);
            Self::process_edge(obj.get_regexp_flags(), vm, stack, live_strings, live_bigints);
        }
        // 遍历 upvalue cell 中的引用。
        for cell_ptr in obj.upvalues_slice() {
            if cell_ptr.is_null() {
                continue;
            }
            let cell = unsafe { &**cell_ptr };
            Self::process_edge(cell.value, vm, stack, live_strings, live_bigints);
        }
    }

    #[inline]
    fn process_edge(
        value: JsValue, vm: &Vm, stack: &mut Vec<*mut JsObject>,
        live_strings: &mut HashSet<*mut JsString, FxBuildHasher>,
        live_bigints: &mut HashSet<*mut num_bigint::BigInt, FxBuildHasher>,
    ) {
        if value.is_object() {
            let ptr = value.as_js_object_ptr();
            // session 边与 epoch 边同栈：epoch 对象的 GC_MARK 位与 session 同字节
            // 可置可读，晋升收集按"标记 ∩ epoch 表"取活集。
            if vm.is_session_ptr(ptr) {
                stack.push(ptr);
            } else if !ptr.is_null() {
                // SAFETY: 执行核心产出的对象值，指针在 session 生命周期内有效。
                if unsafe { (&*ptr).is_epoch() } {
                    stack.push(ptr);
                }
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
    }

    /// 从 VM roots 标记存活的 session/epoch 对象、字符串与 BigInt，供 `sweep` 判定。
    ///
    /// 对象 DFS 同时跟踪 session 与 epoch 两张表，两族 mark 位同字节，晋升收集按
    /// “标记 ∩ epoch 表”取活集；字符串边走 rope 闭包传播（Cons 子节点与扁平化产物），
    /// BigInt 边直接入存活集。roots 由 `Vm::for_each_root` 枚举，字段清单与
    /// `rewrite_values` 一一对应、须同步。只置位不搬移，调用前须先 `clear_all_marks` 清位。
    pub(crate) fn mark(&mut self, vm: &Vm) {
        vm_debug!("[GC] mark phase: {} roots", vm.gc_state.session_object_ptrs.len());
        let mut seeds = Vec::new();
        let mut string_seeds: Vec<*mut JsString> = Vec::new();
        let mut bigint_seeds: Vec<*mut num_bigint::BigInt> = Vec::new();
        vm.for_each_root(|root| {
            if root.is_object() {
                seeds.push(root.as_js_object_ptr());
            } else if root.is_string() {
                string_seeds.push(root.as_string_ptr_mut());
            } else if root.is_bigint() {
                bigint_seeds.push(root.as_bigint_ptr() as *mut num_bigint::BigInt);
            }
        });

        let Self {
            mark_stack: stack,
            live_strings,
            live_bigints,
            ..
        } = self;
        stack.clear();
        live_strings.clear();
        live_bigints.clear();
        for ptr in string_seeds {
            Self::mark_string_live(live_strings, ptr);
        }
        for ptr in bigint_seeds {
            live_bigints.insert(ptr);
        }

        for ptr in seeds {
            if ptr.is_null() {
                continue;
            }
            if vm.is_session_ptr(ptr) {
                stack.push(ptr);
                continue;
            }
            // SAFETY: 对象根由 VM 自有的字段与 builtin 对象产生，指针合法。
            unsafe {
                let obj = &*ptr;
                // epoch 根（顶层 var 寄存器等）自身入栈置位：晋升收集按标记位
                // 判活，根不置位会被误判为死。
                if obj.is_epoch() {
                    stack.push(ptr);
                    continue;
                }
                // P 对象根（builtin world / global）不回收，只扫边。
                Self::scan_edges_for_mark(obj, vm, stack, live_strings, live_bigints);
            }
        }

        while let Some(ptr) = stack.pop() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 由根/session 边发现，session 根检查保证它是合法 session 对象指针。
            unsafe {
                let obj = &mut *ptr;
                if obj.is_gc_marked() {
                    continue;
                }
                obj.set_gc_mark(true);
                Self::scan_edges_for_mark(obj, vm, stack, live_strings, live_bigints);
            }
        }
    }

    /// 释放对象本体之外的堆外属性数据（元素区、元素 meta、hash 属性区、属性 meta
    /// 与各族 native 状态盒），按 capacity 经 `Box::from_raw` 各恰好释放一次，返回
    /// 字节数。`obj_ptr` 为空时返回 0；`require_session` 为 true 时断言对象属
    /// session epoch，epoch 原件走 false 分支。不含 upvalue 列表与对象本体，由调用方处理。
    pub(crate) fn drop_object_heap_data(obj_ptr: *mut JsObject, require_session: bool) -> u64 {
        if obj_ptr.is_null() {
            return 0;
        }
        // SAFETY: `obj_ptr` 在调用本辅助函数前已校验，指向 VM session arena 拥有的
        // session 对象。只重建 JsObject::ensure_hash_props/ensure_prop_meta 分配的 Box，
        // 且只在这里释放一次。
        unsafe {
            let obj = &mut *obj_ptr;
            if require_session {
                debug_assert!(obj.is_session_epoch());
            }
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

            freed_bytes
        }
    }

    fn drop_session_object_heap_data(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_object_heap_data(obj_ptr, true)
    }

    /// 释放一个死 session 对象：对象本体 + 堆数据 + upvalue 列表，返回释放字节数。
    ///
    /// # 注意事项
    /// 仅 sweep 死分支到达此处：只有存活对象被克隆、克隆与原件共享同一 upvalues
    /// Box，死对象从不被克隆，其 upvalue 列表 Box 无其他持有者，在此恰好释放
    /// 一次；存活分支绝不释放（克隆仍引用同一 Box）。释放后置空：死对象已移出
    /// 对象表，收尾统一释放按表枚举不会再见，置空保证对象侧幂等、无陈旧指针。
    fn drop_dead_session_object(obj_ptr: *mut JsObject) -> u64 {
        let mut freed = Self::drop_session_object_heap_data(obj_ptr) + size_of::<JsObject>() as u64;
        // SAFETY: obj_ptr 来自 session 对象表，sweep 期间仍指向旧 arena 内合法对象；
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

    /// 移动式清扫 session 对象：存活对象复制进新 arena，死对象原地释放。
    ///
    /// 按 GC mark 位分流：存活对象经 `clone_for_session_epoch` 复制（native 盒随族深拷）
    /// 并登记 forwarding 旧址→新址，死对象释放本体、堆区与独占 upvalue；随后按转发表重写
    /// 新对象的值边、native 边与 promise 结算链裸指针，转发查找使共享与环去重到同一克隆。
    /// 最后重写全部根引用、换表并以新 Bump 接管 session epoch，旧 arena 整体归还并清 mark 位。
    pub(crate) fn sweep(&mut self, vm: &mut Vm) -> u64 {
        let old_ptrs = std::mem::take(&mut vm.gc_state.session_object_ptrs);
        let mut forwarding = std::mem::take(&mut vm.gc_state.forwarding);
        let new_arena = bumpalo::Bump::new();
        let mut survivors = 0u64;
        let mut dead = 0u64;
        let mut freed_bytes = 0u64;

        for old_ptr in old_ptrs {
            if old_ptr.is_null() {
                continue;
            }
            // SAFETY: old_ptr 来自 session_arena 的晋升，sweep 运行期间仍指向旧 session arena。
            let is_live = unsafe { (*old_ptr).is_gc_marked() };
            if is_live {
                survivors += 1;
                let old_ref = unsafe { &*old_ptr };
                let clone = old_ref.clone_for_session_epoch();
                let new_ptr = new_arena.alloc(clone) as *mut JsObject;
                let new_ref = unsafe { &mut *new_ptr };
                if old_ref.is_map() {
                    map::clone_map_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_set() {
                    set::clone_set_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_weak_map_obj() {
                    // 弱键原样搬运：键生死按转发表收敛后的判定（phase 2 与
                    // 晋升定夺路径同判据），此处只深拷贝盒与改写值边。
                    weak_map::clone_weak_map_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_disposable_stack_obj() || old_ref.is_async_disposable_stack_obj() {
                    disposable_stack::clone_dispose_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_array_buffer_obj() {
                    array_buffer::clone_array_buffer_native(old_ref, new_ref);
                } else if old_ref.is_shared_array_buffer_obj() {
                    array_buffer::clone_shared_array_buffer_native(old_ref, new_ref);
                } else if old_ref.is_typed_array_obj() {
                    typed_array::clone_typed_array_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_data_view_obj() {
                    data_view::clone_data_view_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_promise_obj() {
                    crate::promise::clone_promise_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_async_obj() {
                    crate::async_func::clone_async_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_async_generator_obj() {
                    crate::async_generator::clone_async_generator_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_generator_obj() {
                    crate::generator::clone_generator_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.holds_compiled_regex() {
                    // 已编译正则是 Box 深拷贝到新对象：源 Box 由 drop 释放，互不共享。
                    regexp::clone_regexp_native(old_ref, new_ref);
                } else if old_ref.is_module_namespace() {
                    // 条目表深拷贝：`clone_for_session_epoch` 只浅拷贝 native_data 指针。
                    module::clone_module_ns_native_with_rewrite(old_ref, new_ref, |value| value);
                }
                forwarding.insert(old_ptr, new_ptr);
                freed_bytes += Self::drop_session_object_heap_data(old_ptr);
            } else {
                dead += 1;
                freed_bytes += Self::drop_dead_session_object(old_ptr);
            }
        }

        for &dst in forwarding.values() {
            // SAFETY: forwarding 中的指针全是新分配且已初始化的对象。
            let obj = unsafe { &mut *dst };
            obj.rewrite_object_values(|value| {
                if value.is_object() {
                    let ptr = value.as_js_object_ptr();
                    if let Some(&fwd) = forwarding.get(&ptr) {
                        return JsValue::from_js_object(fwd);
                    }
                }
                value
            });
            if obj.is_map() {
                map::rewrite_map_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_set() {
                set::rewrite_set_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_weak_map_obj() {
                // 键按转发表 + mark 位定生死（死键条目丢弃），值走强边转发改写。
                weak_map::rewrite_weak_map_native(
                    obj,
                    |key| resolve_weak_key_sweep(key, &forwarding),
                    |value| rewrite_forwarded_value(value, &forwarding),
                );
            } else if obj.is_disposable_stack_obj() || obj.is_async_disposable_stack_obj() {
                disposable_stack::rewrite_dispose_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_typed_array_obj() {
                typed_array::rewrite_typed_array_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_data_view_obj() {
                data_view::rewrite_data_view_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_generator_obj() {
                crate::generator::rewrite_generator_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_promise_obj() {
                crate::promise::rewrite_promise_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
                // 结算链指针是裸指针边（非 JsValue）：搬移换址后按转发表重定位，
                // 旧克隆的结算传导仍指向后继克隆的新址。
                crate::promise::repoint_promise_promoted_clone(obj, |ptr| {
                    rewrite_forwarded_value(JsValue::from_js_object(ptr), &forwarding).as_js_object_ptr()
                });
            } else if obj.is_async_obj() {
                crate::async_func::rewrite_async_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_async_generator_obj() {
                crate::async_generator::rewrite_async_generator_native(obj, |value| {
                    rewrite_forwarded_value(value, &forwarding)
                });
            } else if obj.is_module_namespace() {
                module::rewrite_module_ns_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            }
        }

        rewrite_vm_roots(vm, &forwarding);

        vm.gc_state.session_object_ptrs = forwarding.values().copied().collect();
        forwarding.clear();
        vm.gc_state.forwarding = forwarding;
        vm.gc_state.session_epoch = new_arena;
        vm.gc_state.session_bytes_allocated = vm
            .gc_state
            .session_object_ptrs
            .iter()
            .filter(|&&ptr| !ptr.is_null())
            .map(|&ptr| {
                let obj = unsafe { &*ptr };
                size_of::<JsObject>() as u64 + Self::object_heap_data_bytes(obj)
            })
            .sum::<u64>() as usize;

        self.clear_all_marks(vm);

        let total_ptrs = survivors + dead;
        if total_ptrs > 0 {
            if dead == 0 {
                vm_debug!("[GC] sweep phase -> no objects collected ({} live, {} dead)", survivors, dead);
            } else {
                vm_debug!(
                    "[GC] sweep phase: {} scanned, {} live, {} dead, {} bytes",
                    total_ptrs,
                    survivors,
                    dead,
                    freed_bytes
                );
            }
        }

        self.total_bytes_freed += freed_bytes;
        self.total_objects_scanned += total_ptrs;
        self.total_objects_live += survivors;
        self.total_objects_dead += dead;
        self.last_collection_objects_scanned = total_ptrs;
        self.last_collection_objects_live = survivors;
        self.last_collection_objects_dead = dead;
        self.last_collection_bytes_freed = freed_bytes;
        freed_bytes
    }

    /// 清扫 session `JsString`：保留 `mark()` 阶段记为存活的部分，其余经
    /// `Box::from_raw` 释放。存活字符串不被搬移——Box 地址稳定——因此无需
    /// forwarding 表与根指针重写。在对象清扫之后运行（对象清扫会把
    /// `session_bytes_allocated` 重置为仅对象），再补回存活字符串字节。
    /// 返回释放的字节数。
    pub(crate) fn sweep_session_strings(&mut self, vm: &mut Vm) -> u64 {
        vm_debug!("[GC] sweep strings: {} string ptrs", vm.gc_state.session_string_ptrs.len());
        let old = std::mem::take(&mut vm.gc_state.session_string_ptrs);
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
        vm.gc_state.session_string_ptrs = live;
        vm.gc_state.session_bytes_allocated = vm.gc_state.session_bytes_allocated.saturating_add(live_bytes);

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    /// 清扫 session `BigInt`：保留 `mark()` 阶段记为存活的部分，其余经
    /// `Box::from_raw` 释放。存活 BigInt 不被搬移——Box 地址稳定——因此无需
    /// forwarding 表与根指针重写。在字符串清扫之后运行。
    /// 返回释放的字节数。
    pub(crate) fn sweep_session_bigints(&mut self, vm: &mut Vm) -> u64 {
        let old = vm.gc_state.session_bigint_ptrs.borrow_mut().drain(..).collect::<Vec<_>>();
        let mut freed = 0u64;
        let mut live_bytes = 0usize;
        let mut live = Vec::with_capacity(old.len());
        for ptr in old {
            if ptr.is_null() {
                continue;
            }
            if self.live_bigints.contains(&ptr) {
                // 存活——地址不变，无需重写。
                live_bytes += size_of::<num_bigint::BigInt>();
                live.push(ptr);
            } else {
                // SAFETY: ptr 在 session_bigint_ptrs 中但不可达，恰好释放一次。
                freed += size_of::<num_bigint::BigInt>() as u64;
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
        *vm.gc_state.session_bigint_ptrs.borrow_mut() = live;
        vm.gc_state.session_bytes_allocated = vm.gc_state.session_bytes_allocated.saturating_add(live_bytes);

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    /// 完整收集的触发判断：对象、字符串或 BigInt 表非空，且字节账目已达阈值。
    pub(crate) fn should_collect(&self, vm: &Vm) -> bool {
        (!vm.gc_state.session_object_ptrs.is_empty()
            || !vm.gc_state.session_string_ptrs.is_empty()
            || !vm.gc_state.session_bigint_ptrs.borrow().is_empty())
            && vm.gc_state.session_bytes_allocated >= vm.kernel_core().config().session_gc_threshold
    }

    /// 执行期字符串回收的触发判断：账目须超过水位（上次收集后的存活字节 +
    /// 阈值增量）。完整收集（reset）仍用 [`Self::should_collect`] 的阈值直接比较，
    /// 保证死对象超阈值即被 reset 回收；执行期走增量水位，活串超阈值时不每指令
    /// 重复触发无死串可回收的白跑。
    pub(crate) fn should_collect_strings(&self, vm: &Vm) -> bool {
        (!vm.gc_state.session_object_ptrs.is_empty()
            || !vm.gc_state.session_string_ptrs.is_empty()
            || !vm.gc_state.session_bigint_ptrs.borrow().is_empty())
            && vm.gc_state.session_bytes_allocated >= vm.gc_state.string_gc_watermark
    }

    /// debug 兜底：mark 之后断言所有存活对象持有的字符串边都已登记进
    /// `live_strings`——捕获"对象边漏登记 → 存活串被误释放"的静默悬垂。
    #[cfg(debug_assertions)]
    fn debug_assert_marked_object_strings_live(&self, vm: &Vm) {
        let mut live = HashSet::with_hasher(FxBuildHasher);
        for &ptr in &vm.gc_state.session_object_ptrs {
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

    /// 完整收集编排：`mark` → `sweep`（对象移动式搬移）→ `sweep_session_strings` →
    /// `sweep_session_bigints`，累加三类释放字节；字符串与 BigInt 地址稳定、无需
    /// forwarding。更新各表、字节账目与 mark 位，累计收集次数与最近/最大/最小耗时，
    /// 释放字节大于 0 或每满 100 次时输出统计摘要。
    pub(crate) fn collect(&mut self, vm: &mut Vm) {
        let start = Instant::now();

        vm_info!("[GC] cycle #{} start", self.total_collections + 1);

        self.mark(vm);
        let mut freed_bytes = self.sweep(vm);
        freed_bytes += self.sweep_session_strings(vm);
        freed_bytes += self.sweep_session_bigints(vm);

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
    /// 刻意跳过对象 sweep 搬移——builtin/dispatch 层持有跨分配点的 session 对象
    /// 裸指针，移动式 sweep 会使其悬垂；对象执行期回收仍只在 reset 统一进行。
    /// 字符串每串独立 Box、地址稳定，清扫无需 forwarding 与根重写。
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
    ///   由 `sweep_session_strings` 补回存活串字节，与完整收集的最终账目一致。
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
            .gc_state
            .session_object_ptrs
            .iter()
            .filter(|&&ptr| !ptr.is_null())
            .map(|&ptr| {
                let obj = unsafe { &*ptr };
                size_of::<JsObject>() as u64 + Self::object_heap_data_bytes(obj)
            })
            .sum::<u64>() as usize;
        vm.gc_state.session_bytes_allocated = object_bytes;
        let mut freed_bytes = self.sweep_session_strings(vm);
        freed_bytes += self.sweep_session_bigints(vm);

        // 恢复 mark 位不变量：mark 之后必由清位收尾（与 sweep 末尾同点清位）。
        // 残留 marked 对象会让下一次完整收集（reset 路径）的 mark DFS 短路漏标，
        // 其字符串边不进 live_strings → 存活串被误释放 → 存活对象持悬垂指针。
        self.clear_all_marks(vm);

        // 抬高下次触发水位：本次存活字节 + 阈值增量，避免活串超阈值时每指令重复
        // 触发无死串可回收的完整 mark + 串清扫。
        let threshold = vm.kernel_core().config().session_gc_threshold;
        vm.gc_state.string_gc_watermark = vm.gc_state.session_bytes_allocated.saturating_add(threshold);

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

    /// 执行期两档收集：epoch 晋升档 + session 原地非移动 sweep 档。
    ///
    /// 在 dispatch 安全点（循环顶 + `native_call_depth == 0`）回收单 run 分配
    /// 包络内的死对象：
    /// - epoch 晋升档：扩展 mark（epoch 臂跟随）定活集，活 epoch 对象晋升
    ///   session（递归克隆 + forwarding 去重共享与环 + native 盒深拷），根与
    ///   session 对象的 epoch 子引用按转发表改写，全部 epoch 旧堆区与独占
    ///   upvalue 列表释放，epoch 换新 Bump；
    /// - session 原地 sweep 档：死 session 对象原地释放（堆区 + upvalue 列表）
    ///   并出表；存活对象不移动——地址不变、免 forwarding/rewrite，用户可观察
    ///   identity 不分裂。
    ///
    /// # 边界与前提
    /// - 调用方已完成门控（无活跃/挂起 for-in）：ForInIter body 分配于 epoch
    ///   arena，换新 Bump 即时失效在表迭代器；
    /// - 调用点为 dispatch 安全点（`native_call_depth == 0`），无在途 builtin
    ///   局部裸指针、dispatch 未重入。
    ///
    /// # 副作用
    /// - `epoch_object_ptrs` 清空、epoch Bump 换新（旧 arena 全量归还）；
    /// - `session_object_ptrs` 仅剩存活，`session_bytes_allocated` 按存活重算
    ///   （串/BigInt 归 strings-only 路径，本路径不动，仅对象口径变化）；
    /// - `gc_watermark` = 当前分配包络 + 阈值增量；
    /// - 累计/时长统计更新；mark 位全清（收集后无残留）。
    pub(crate) fn collect_in_run(&mut self, vm: &mut Vm) {
        let start = Instant::now();
        vm_info!("[GC] in-run cycle #{} start", self.total_collections + 1);

        // 清残留 mark 位（session + epoch）：防 mark DFS 因历史 true 短路漏标。
        self.clear_all_marks(vm);

        // 扩展 mark：epoch 臂跟随，活 epoch 集 = 被标记 ∩ epoch 表。
        self.mark(vm);

        // ── epoch 晋升档 ──

        let mut forwarding = std::mem::take(&mut vm.gc_state.forwarding);
        let epoch_ptrs = std::mem::take(&mut vm.gc_state.epoch_object_ptrs);

        // 活 epoch 对象逐个晋升：递归克隆携带 native 盒，共享/环经 forwarding
        // 去重；死对象留在 epoch arena，换新 Bump 前统一释放。
        let mut live_epoch = 0u64;
        for &ptr in &epoch_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 epoch 对象表登记，arena 存活期内有效。
            if unsafe { (*ptr).is_gc_marked() } {
                vm.promote_object_inner(ptr, &mut forwarding);
                live_epoch += 1;
            }
        }

        // 晋升克隆体自身即活：克隆不携带源对象标记位，而紧随其后的 session
        // 原地 sweep 按标记位保活——不置位则克隆体在同一轮内被当作死对象
        // 原地释放，而根已改写指向克隆体（悬垂）。此点转发表值集恰为本轮
        // 晋升克隆体全集（session 原地改写路径的补晋升发生在其后、不入
        // 本轮 sweep 集，无需置位）。
        for &dst in forwarding.values() {
            // SAFETY: dst 为本轮晋升克隆体，session arena 存活期内有效。
            unsafe { (*dst).set_gc_mark(true) };
        }

        // 根持有的 epoch 对象（顶层 var 寄存器等）按转发表解析到克隆体。
        rewrite_vm_roots(vm, &forwarding);

        // session 对象可持有绕过写屏障的 epoch 子引用（函数 captured_this/
        // home_object、native 盒直插）：按转发表就地晋升进 session（已晋升
        // 对象的子引用解析到同一克隆，无二次克隆）。只改写存活对象：死对象的
        // 子引用随对象原地释放（释放路径无后续解引用，不悬垂）。
        let session_ptrs = std::mem::take(&mut vm.gc_state.session_object_ptrs);
        let marked_session: Vec<*mut JsObject> = session_ptrs
            .iter()
            .copied()
            .filter(|&ptr| !ptr.is_null() && unsafe { (*ptr).is_gc_marked() })
            .collect();
        vm.rewrite_session_epoch_refs(&marked_session, &mut forwarding);
        // 弱表键定夺：主改写只走强边，epoch 键按收敛后的转发表定生死。
        vm.rewrite_weak_map_keys_after_promotion(&marked_session, &mut forwarding);
        forwarding.clear();
        vm.gc_state.forwarding = forwarding;

        // 全部 epoch 旧堆区释放：活对象的 vec/native 盒已被克隆深拷贝取代，
        // 原件原地释放；死对象堆区随本体原地释放。
        let epoch_freed = Self::free_epoch_object_heap_data(&epoch_ptrs, &session_ptrs);

        // 换新 epoch Bump：旧 arena 全量归还，容量不保留。
        vm.epoch.reset();

        // ── session 原地非移动 sweep 档 ──

        // 死 session 对象原地释放（堆区 + upvalue 列表）并出表；存活对象不动
        // arena 槽位——地址不变，免 forwarding/rewrite。
        let mut survivors = Vec::with_capacity(session_ptrs.len());
        let mut dead_session = 0u64;
        let mut session_freed = 0u64;
        for &ptr in &session_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session 对象表登记，arena 存活期内有效。
            if unsafe { (*ptr).is_gc_marked() } {
                survivors.push(ptr);
            } else {
                session_freed += Self::drop_dead_session_object(ptr);
                dead_session += 1;
            }
        }
        let live_session = survivors.len() as u64;
        vm.gc_state.session_object_ptrs = survivors;

        // 恢复 mark 位不变量：收集后无残留（残留 true 使下一次 mark DFS 短路漏标）。
        self.clear_all_marks(vm);

        // 对象口径重算：存活对象 + 存活串 + BigInt（串/BigInt 本路径不动，
        // 随公式整体重算保持与 strings-only 口径一致）。
        let mut object_bytes: usize = 0;
        for &ptr in &vm.gc_state.session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: 存活对象在 session 表中，arena 存活期内有效。
            let obj = unsafe { &*ptr };
            object_bytes += size_of::<JsObject>() + Self::object_heap_data_bytes(obj) as usize;
        }
        let mut string_bytes: usize = 0;
        for &ptr in &vm.gc_state.session_string_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 在字符串表登记，收尾前有效。
            string_bytes += size_of::<JsString>() + unsafe { (*ptr).payload_bytes() };
        }
        let bigint_bytes = vm.gc_state.session_bigint_ptrs.borrow().len() * size_of::<num_bigint::BigInt>();
        vm.gc_state.session_bytes_allocated = object_bytes + string_bytes + bigint_bytes;

        // 抬高下次触发水位：当前分配包络 + 阈值增量——存活包络超阈值时不每指令
        // 重复触发无死对象可回收的白跑。
        vm.gc_state.gc_watermark = vm.run_alloc_bytes().saturating_add(vm.gc_state.gc_threshold_cached);
        vm.gc_state.gc_gate_retry_alloc = 0;

        let live = live_epoch + live_session;
        let dead = (epoch_ptrs.len() as u64 - live_epoch) + dead_session;
        let freed_bytes = epoch_freed + session_freed;

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);
        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed_bytes);
        self.total_objects_scanned += (epoch_ptrs.len() + session_ptrs.len()) as u64;
        self.total_objects_live += live;
        self.total_objects_dead += dead;
        self.last_collection_objects_scanned = (epoch_ptrs.len() + session_ptrs.len()) as u64;
        self.last_collection_objects_live = live;
        self.last_collection_objects_dead = dead;
        self.last_collection_bytes_freed = freed_bytes;

        vm_info!(
            "[GC] in-run cycle #{} end: {} scanned, {} live, {} dead, {:.1}ms, {} bytes freed",
            self.total_collections,
            self.last_collection_objects_scanned,
            self.last_collection_objects_live,
            self.last_collection_objects_dead,
            elapsed.as_secs_f64() * 1000.0,
            freed_bytes,
        );
    }

    /// 释放全部 epoch 对象的旧堆区（四处属性/元素向量 + native 状态盒）与独占
    /// upvalue 列表，返回释放字节数。
    ///
    /// 调用前提：活 epoch 对象已晋升 session——其 vec/native 盒被克隆深拷贝取代，
    /// 原件在此恰好释放一次；死对象堆区随本体原地释放。upvalue 列表原件与晋升
    /// 克隆共享同一 Box（`clone_for_session_epoch` 别名）：共享项由 session 对象
    /// 持有、留待收尾统一释放，此处只放独占项，保证恰好一次。
    fn free_epoch_object_heap_data(epoch_ptrs: &[*mut JsObject], session_ptrs: &[*mut JsObject]) -> u64 {
        // 共享集：session 对象表（存活与待死同表）仍持有的 upvalue Box——
        // 待死 session 对象的 Box 由本收集的原地 sweep 释放，交点恰好一处。
        let mut shared: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &ptr in session_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session 对象表登记，arena 存活期内有效。
            let up = unsafe { (*ptr).upvalues } as usize;
            if up != 0 {
                shared.insert(up);
            }
        }

        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut freed = 0u64;
        for &ptr in epoch_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: epoch 对象表未清空，指向 epoch arena 内合法对象。
            unsafe {
                freed += Self::drop_object_heap_data(ptr, false);
                let up = (*ptr).upvalues as usize;
                if up == 0 || shared.contains(&up) || !seen.insert(up) {
                    continue;
                }
                // SAFETY: up 由 set_upvalues 的 Box::into_raw 分配，去重与共享集
                // 保证本路径恰好释放一次。
                drop(Box::from_raw(up as *mut Vec<*mut oxide_types::object::Cell>));
                (*ptr).upvalues = std::ptr::null_mut();
            }
        }
        freed
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
            mark_stack: Vec::new(),
            live_strings: HashSet::default(),
            live_bigints: HashSet::default(),
        }
    }
}

fn rewrite_forwarded_value(
    value: JsValue, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
) -> JsValue {
    if !value.is_object() {
        return value;
    }
    forwarding
        .get(&value.as_js_object_ptr())
        .map(|&ptr| JsValue::from_js_object(ptr))
        .unwrap_or(value)
}

/// 移动式 sweep 改写期的弱键判定：转发表内键改指新址；未入表的 session/
/// epoch 键按 mark 位定生死（已标 = 强可达、随后晋升或原地存活，未标 =
/// 死键丢弃）；P 键不可死，恒保留。
///
/// # 边界与前提
/// - 未入表键读归属位与 mark 位须解引用旧指针：调用点（sweep 改写相）
///   旧 arena 尚未归还、清位发生在改写相之后，对象头位域可读。
/// - 死键指针只作哈希键与位域读取，不 deref 其已释放的堆数据。
pub(crate) fn resolve_weak_key_sweep(
    key: weak_map::WeakKey, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
) -> Option<weak_map::WeakKey> {
    let weak_map::WeakKey::Obj(ptr) = key else {
        return Some(key);
    };
    let mut_ptr = ptr as *mut JsObject;
    if let Some(&new) = forwarding.get(&mut_ptr) {
        return Some(weak_map::WeakKey::Obj(new as *const JsObject));
    }
    // SAFETY: 见函数边界与前提——sweep 改写相旧 arena 存活，对象头位域可读。
    let old = unsafe { &*mut_ptr };
    if (old.is_session_epoch() || old.is_epoch()) && !old.is_gc_marked() {
        return None;
    }
    Some(key)
}

/// 晋升收敛后的弱键定夺：转发表内键改指新址；未入表 epoch 键 = 本轮晋升
/// 未覆盖 = 死键丢弃；session 键不搬移、原样保留（惰性判定交下一轮 sweep）；
/// P 键不可死，恒保留。
///
/// # 边界与前提
/// - 晋升定夺路径（原地晋升 / in-run 晋升档）的转发表已收敛为最终强可达集，
///   判据不含 mark 位（reset 边界路径此前已清位）。
/// - 解引用仅限未入表键的归属位读取，调用点旧 arena 存活（epoch 释放 /
///   清表发生在定夺之后）。
pub(crate) fn resolve_weak_key_after_promotion(
    key: weak_map::WeakKey, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
) -> Option<weak_map::WeakKey> {
    let weak_map::WeakKey::Obj(ptr) = key else {
        return Some(key);
    };
    let mut_ptr = ptr as *mut JsObject;
    if let Some(&new) = forwarding.get(&mut_ptr) {
        return Some(weak_map::WeakKey::Obj(new as *const JsObject));
    }
    // SAFETY: 见函数边界与前提——定夺点旧 arena 存活，对象头位域可读。
    let old = unsafe { &*mut_ptr };
    if old.is_epoch() {
        return None;
    }
    Some(key)
}

/// 按 forwarding 表把全部 VM 根引用重写到搬移后的新地址。与 `for_each_value`
/// 共用同一字段清单（经 `rewrite_values` 遍历）且须同步：遗漏字段会在搬移后
/// 保留指向旧 arena 的悬垂指针。
pub(crate) fn rewrite_vm_roots(vm: &mut Vm, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>) {
    vm_debug!("[GC] rewrite_vm_roots: {} forwarded objects", forwarding.len());
    // 统一遍历：与 for_each_value 共用同一字段清单。
    vm.rewrite_values(|value| rewrite_forwarded_value(value, forwarding));
}

#[cfg(test)]
mod tests;
