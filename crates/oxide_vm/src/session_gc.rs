use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::time::Instant;

use crate::{vm_debug, vm_info};
use oxide_types::object::{JsObject, JsString, PropMetaEntry};
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::vm::Vm;
use oxide_builtins::{array_buffer, data_view, disposable_stack, map, regexp, set, typed_array};

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
    }

    /// 只读核算 session 对象的堆数据字节（属性/元素/meta Vec capacity + native 状态盒）。
    /// 与 `drop_object_heap_data` 释放口径一致（capacity），不释放、不置空任何指针。
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

        bytes += map::map_native_size(obj);
        bytes += set::set_native_size(obj);
        bytes += disposable_stack::disposable_stack_native_size(obj);
        bytes += array_buffer::array_buffer_native_size(obj);
        bytes += regexp::regexp_native_size(obj);
        bytes += typed_array::typed_array_native_size(obj);
        bytes += data_view::data_view_native_size(obj);
        bytes += crate::generator::generator_native_size(obj);
        bytes += crate::promise::promise_native_size(obj);
        bytes += crate::async_func::async_native_size(obj);
        bytes += crate::async_generator::async_generator_native_size(obj);

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
            if vm.is_session_ptr(ptr) {
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
    }

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
            // SAFETY: 对象根由 VM 自有的字段与 builtin 对象产生。
            unsafe {
                let obj = &*ptr;
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
            freed_bytes += disposable_stack::drop_dispose_native(obj);
            freed_bytes += array_buffer::drop_array_buffer_native(obj);
            freed_bytes += regexp::drop_regexp_native(obj);
            freed_bytes += typed_array::drop_typed_array_native(obj);
            freed_bytes += data_view::drop_data_view_native(obj);
            freed_bytes += crate::generator::drop_generator_native(obj);
            freed_bytes += crate::promise::drop_promise_native(obj);
            freed_bytes += crate::async_func::drop_async_native(obj);
            freed_bytes += crate::async_generator::drop_async_generator_native(obj);

            freed_bytes
        }
    }

    fn drop_session_object_heap_data(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_object_heap_data(obj_ptr, true)
    }

    fn drop_dead_session_object(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_session_object_heap_data(obj_ptr) + size_of::<JsObject>() as u64
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
        let bytes = (size_of::<JsString>() + (*ptr).len()) as u64;
        Self::drop_session_string_box(ptr);
        bytes
    }

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
                } else if old_ref.is_disposable_stack_obj() || old_ref.is_async_disposable_stack_obj() {
                    disposable_stack::clone_dispose_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_array_buffer_obj() {
                    array_buffer::clone_array_buffer_native(old_ref, new_ref);
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
                } else if old_ref.is_regexp_obj() {
                    // 已编译正则是 Box 深拷贝到新对象：源 Box 由 drop 释放，互不共享。
                    regexp::clone_regexp_native(old_ref, new_ref);
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
            } else if obj.is_async_obj() {
                crate::async_func::rewrite_async_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_async_generator_obj() {
                crate::async_generator::rewrite_async_generator_native(obj, |value| {
                    rewrite_forwarded_value(value, &forwarding)
                });
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
                live_bytes += unsafe { size_of::<JsString>() + (*ptr).len() };
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

    pub(crate) fn should_collect(&self, vm: &Vm) -> bool {
        (!vm.gc_state.session_object_ptrs.is_empty()
            || !vm.gc_state.session_string_ptrs.is_empty()
            || !vm.gc_state.session_bigint_ptrs.borrow().is_empty())
            && vm.gc_state.session_bytes_allocated >= vm.kernel_core().config().session_gc_threshold
    }

    /// 执行期完整 GC 的触发判断：账目须超过水位（上次收集后的存活字节 +
    /// 阈值增量）。与 `should_collect`（reset 路径用）不同，本方法走增量水位，
    /// 防止存活对象超阈值时每指令重复触发无死对象可回收的白跑。
    /// 预留给 17.3b（对象侧执行期触发）安全点审计后使用。
    #[allow(dead_code)]
    pub(crate) fn should_collect_gc(&self, vm: &Vm) -> bool {
        (!vm.gc_state.session_object_ptrs.is_empty()
            || !vm.gc_state.session_string_ptrs.is_empty()
            || !vm.gc_state.session_bigint_ptrs.borrow().is_empty())
            && vm.gc_state.session_bytes_allocated >= vm.gc_state.gc_watermark
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

    pub(crate) fn maybe_collect_strings_only(&mut self, vm: &mut Vm) {
        if self.should_collect_strings(vm) {
            self.collect_strings_only(vm);
        }
    }

    pub(crate) fn maybe_collect(&mut self, vm: &mut Vm) {
        if self.should_collect(vm) {
            self.collect(vm);
        }
    }

    /// 执行期完整 GC 入口：按水位判定，触发后抬高水位。
    /// 预留给 17.3b（对象侧执行期触发）安全点审计后使用。
    #[allow(dead_code)]
    pub(crate) fn maybe_collect_gc(&mut self, vm: &mut Vm) {
        if self.should_collect_gc(vm) {
            self.collect(vm);
            // 抬高下次触发水位：存活字节 + 阈值增量，避免活对象超阈值时每指令重复触发。
            let threshold = vm.gc_state.gc_threshold_cached;
            vm.gc_state.gc_watermark = vm.gc_state.session_bytes_allocated.saturating_add(threshold);
        }
    }

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

fn rewrite_vm_roots(vm: &mut Vm, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>) {
    vm_debug!("[GC] rewrite_vm_roots: {} forwarded objects", forwarding.len());
    // 统一遍历：与 for_each_value 共用同一字段清单。
    vm.rewrite_values(|value| rewrite_forwarded_value(value, forwarding));
}

#[cfg(test)]
mod tests {
    use oxide_compiler::compiler::Compiler;
    use oxide_kernel::kernel::{KernelConfig, KernelCore};
    use oxide_parser::Allocator;
    use oxide_types::object::JsObject;
    use oxide_types::value::JsValue;

    use super::*;
    use crate::vm::{CallFrame, FrameContinuation};
    use oxide_builtins::{array_buffer, data_view, map, set, typed_array};
    use oxide_runtime_api::NativeResult;

    fn plain_object(vm: &mut Vm) -> *mut JsObject {
        let proto_ptr = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let ptr = vm.epoch.alloc(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*ptr).set_is_epoch(true) };
        ptr
    }

    fn has_ptr(roots: &[JsValue], ptr: *mut JsObject) -> bool {
        roots
            .iter()
            .any(|value| value.is_object() && std::ptr::eq(value.as_js_object_ptr(), ptr))
    }

    #[test]
    fn uncaught_value_is_gc_root() {
        let mut vm = Vm::new();
        let obj = plain_object(&mut vm);
        let session = vm.promote_object(obj);
        vm.last_uncaught_value = Some(JsValue::from_js_object(session));
        let mut roots = Vec::new();
        vm.for_each_root(|v| roots.push(v));
        assert!(has_ptr(&roots, session));
    }

    #[test]
    fn suspended_signal_fields_are_roots() {
        let mut vm = Vm::new();
        let a = plain_object(&mut vm);
        let b = plain_object(&mut vm);
        let c = plain_object(&mut vm);
        let a_s = vm.promote_object(a);
        let b_s = vm.promote_object(b);
        let c_s = vm.promote_object(c);
        vm.generator_suspended = Some(JsValue::from_js_object(a_s));
        vm.async_context = Some(JsValue::from_js_object(b_s));
        vm.async_gen_context = Some(JsValue::from_js_object(c_s));
        let mut roots = Vec::new();
        vm.for_each_root(|v| roots.push(v));
        assert!(has_ptr(&roots, a_s));
        assert!(has_ptr(&roots, b_s));
        assert!(has_ptr(&roots, c_s));
    }

    #[test]
    fn gc_roots_contains_registers_frames_and_root_roots() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let frame_obj = plain_object(&mut vm);
        let saved_this = plain_object(&mut vm);
        let child = plain_object(&mut vm);

        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
        }

        let root_session = vm.promote_object(root);
        let frame_session = vm.promote_object(frame_obj);
        let this_session = vm.promote_object(saved_this);
        let child_session = vm.promote_object(child);
        vm.regs[0] = JsValue::from_js_object(root_session);

        vm.frames.push(CallFrame {
            return_addr: 0,
            function_name: 0,
            caller_reg_limit: 1,
            caller_active_reg_limit: 1,
            saved_reg_offset: 0,
            spill_offset: 0,
            arguments_base: 0,
            arguments_count: 0,
            saved_this: JsValue::from_js_object(this_session),
            saved_new_target: JsValue::from_js_object(child_session),
            callee: JsValue::from_js_object(child_session),
            construct_result_reg: None,
            strict: false,
            constructed_this: Some(JsValue::from_js_object(child_session)),
            is_derived_constructor: false,
            super_called: false,
            continuation: FrameContinuation::None,
        });
        vm.save_stack.push(JsValue::from_js_object(frame_session));

        vm.regs[1] = JsValue::from_js_object(child_session);
        vm.exception_value = Some(JsValue::from_js_object(root_session));
        vm.pending_exception = Some(JsValue::from_js_object(child_session));
        vm.iters.for_of_iters.push(crate::vm_state::ForOfEntry {
            iterator: JsValue::from_js_object(child_session),
            last_result: JsValue::from_js_object(root_session),
            is_async: false,
        });

        let mut roots = Vec::new();
        vm.for_each_root(|v| roots.push(v));
        assert!(has_ptr(&roots, root_session));
        assert!(has_ptr(&roots, frame_session));
        assert!(has_ptr(&roots, this_session));
        assert!(has_ptr(&roots, child_session));
        assert!(has_ptr(&roots, vm.session.global_object().as_ptr() as *mut JsObject));
        assert!(!roots.is_empty());
        assert!(roots.contains(&JsValue::from_js_object(root_session)));
        assert_eq!(vm.exception_value, Some(JsValue::from_js_object(root_session)));
    }

    #[test]
    fn rewrite_matches_roots_coverage() {
        // 指针重写必须覆盖根收集访问的同一组执行核心字段：session→marker 映射后，
        // 每个持有 session 的字段都应被改写为 marker（与 Step 1 的 for_each 覆盖测试
        // 互补，共同构成孪生清单验证）。
        let mut vm = Vm::new();
        let obj = plain_object(&mut vm);
        let session = vm.promote_object(obj);
        let marker = plain_object(&mut vm);
        let marker_session = vm.promote_object(marker);
        vm.regs[0] = JsValue::from_js_object(session);
        vm.last_uncaught_value = Some(JsValue::from_js_object(session));
        vm.generator_suspended = Some(JsValue::from_js_object(session));
        vm.delegated_iterator = Some(JsValue::from_js_object(session));
        vm.async_context = Some(JsValue::from_js_object(session));
        vm.async_gen_context = Some(JsValue::from_js_object(session));
        vm.inline_callee = Some(JsValue::from_js_object(session));
        vm.pending_completion = Some(crate::vm::Completion::Return {
            value: JsValue::from_js_object(session),
            remaining_finally: 0,
            for_of_count: 0,
            for_in_count: 0,
        });

        let mut forwarding = std::collections::HashMap::new();
        forwarding.insert(session, marker_session);
        vm.rewrite_values(|v| {
            if v.is_object() {
                if let Some(&m) = forwarding.get(&v.as_js_object_ptr()) {
                    return JsValue::from_js_object(m);
                }
            }
            v
        });
        assert_eq!(vm.regs[0].as_js_object_ptr(), marker_session);
        assert_eq!(vm.last_uncaught_value.unwrap().as_js_object_ptr(), marker_session);
        assert_eq!(vm.generator_suspended.unwrap().as_js_object_ptr(), marker_session);
        assert_eq!(vm.delegated_iterator.unwrap().as_js_object_ptr(), marker_session);
        assert_eq!(vm.async_context.unwrap().as_js_object_ptr(), marker_session);
        assert_eq!(vm.async_gen_context.unwrap().as_js_object_ptr(), marker_session);
        assert_eq!(vm.inline_callee.unwrap().as_js_object_ptr(), marker_session);
        match vm.pending_completion.unwrap() {
            crate::vm::Completion::Return { value, .. } => assert_eq!(value.as_js_object_ptr(), marker_session),
            _ => panic!("expected Return completion"),
        }
    }

    #[test]
    fn mark_phase_reaches_cycles_and_unreachable_are_unmarked() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let reachable = plain_object(&mut vm);
        let unreachable = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(reachable));
            (*reachable).set_prop_at(0, JsValue::from_js_object(root));
        }

        let root_session = vm.promote_object(root);
        let reachable_session = unsafe { (*root_session).get_prop_at(0).as_js_object_ptr() };
        let unreachable_session = vm.promote_object(unreachable);

        vm.regs[0] = JsValue::from_js_object(root_session);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.mark(&vm);
        vm.gc_state.session_gc = gc;

        assert!(unsafe { (*root_session).is_gc_marked() });
        assert!(unsafe { (*reachable_session).is_gc_marked() });
        assert!(!unsafe { (*unreachable_session).is_gc_marked() });
        assert_ne!(unreachable_session, root_session);
    }

    #[test]
    fn sweep_preserves_cycle_and_collects_unreachable() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let child = plain_object(&mut vm);
        let dead = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
            (*child).set_prop_at(0, JsValue::from_js_object(root));
        }

        let root_session = vm.promote_object(root);
        vm.regs[0] = JsValue::from_js_object(root_session);
        let dead_session = vm.promote_object(dead);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.mark(&vm);
        let _ = gc.sweep(&mut vm);
        vm.gc_state.session_gc = gc;

        assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
        assert!(!vm.gc_state.session_object_ptrs.contains(&root_session));
        assert!(!vm.gc_state.session_object_ptrs.contains(&dead_session));
        assert!(!vm
            .gc_state
            .session_object_ptrs
            .iter()
            .any(|ptr| unsafe { (*(*ptr)).is_gc_marked() }));
    }

    #[test]
    fn sweep_preserves_array_elements_and_collects_dead_element_object() {
        let mut vm = Vm::new();
        let array_proto = vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let arr = vm.epoch.alloc(JsObject::new_array(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(array_proto),
            2,
            vm.epoch.bump(),
        ));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*arr).set_is_epoch(true) };
        let live_elem = plain_object(&mut vm);
        let dead_elem = plain_object(&mut vm);
        unsafe {
            (*arr).set_prop_at(0, JsValue::from_js_object(live_elem));
            (*arr).set_prop_at(1, JsValue::from_js_object(dead_elem));
        }
        // 晋升数组会把元素对象一并带入 session；随后断开元素 1 的引用，使其成为
        // mark 不可达的死对象，供 sweep 回收。
        let arr_session = vm.promote_object(arr);
        unsafe {
            (*arr_session).set_prop_at(1, JsValue::undefined());
        }
        vm.regs[0] = JsValue::from_js_object(arr_session);

        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.mark(&vm);
        let _ = gc.sweep(&mut vm);
        vm.gc_state.session_gc = gc;

        // sweep 复制存活对象并把 VM 根改写为新 arena 指针：regs[0] 是数组的新地址，
        // 旧指针已随旧 arena 释放，不可再用。
        let arr_new = vm.regs[0].as_js_object_ptr();
        let live_session = unsafe { (*arr_new).get_prop_at(0).as_js_object_ptr() };
        assert_eq!(unsafe { (*live_session).prop_count() }, 0);
        assert_eq!(unsafe { (*arr_new).prop_count() }, 2);
        assert_eq!(unsafe { (*arr_new).get_prop_at(0).as_js_object_ptr() }, live_session);
        // 元素 1 引用已断开：死元素对象被回收，存活集合只剩数组与元素 0 的对象。
        assert_eq!(unsafe { (*arr_new).get_prop_at(1) }, JsValue::undefined());
        assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
    }

    #[test]
    fn forwarding_is_cleared_after_sweep() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let child = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
        }
        let root_session = vm.promote_object(root);
        vm.regs[0] = JsValue::from_js_object(root_session);
        let _ = vm.promote_object(child);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.mark(&vm);
        let _ = gc.sweep(&mut vm);
        vm.gc_state.session_gc = gc;

        // 复用的 forwarding 表必须在 sweep 后清空，否则 promote 会观察到指向已释放
        // arena 的过期 old->new 条目。
        assert!(vm.gc_state.forwarding.is_empty());
    }

    fn vm_with_low_threshold() -> Vm {
        let mut cfg = KernelConfig::minimal();
        cfg.set_session_gc_threshold(1);
        let core = KernelCore::new(cfg);
        Vm::with_kernel_core(core)
    }

    fn native_ok(result: NativeResult) -> JsValue {
        match result {
            NativeResult::Ok(value) => value,
            NativeResult::Err(err) => panic!("native error: {err}"),
            NativeResult::TailCall { .. } => panic!("unexpected native bytecode call"),
        }
    }

    /// 分配一个原型指向 Map.prototype 的占位对象并写入寄存器，作为构造器调用的 `this`。
    fn map_this(vm: &mut Vm, reg: u8) -> JsValue {
        let proto = vm.session.builtin_world().map_proto.as_ptr() as *mut JsObject;
        let obj = vm.epoch.alloc(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto),
        ));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*obj).set_is_epoch(true) };
        let val = JsValue::from_js_object(obj);
        vm.regs[reg as usize] = val;
        val
    }

    /// 分配一个原型指向 Set.prototype 的占位对象并写入寄存器，作为构造器调用的 `this`。
    fn set_this(vm: &mut Vm, reg: u8) -> JsValue {
        let proto = vm.session.builtin_world().set_proto.as_ptr() as *mut JsObject;
        let obj = vm.epoch.alloc(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto),
        ));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*obj).set_is_epoch(true) };
        let val = JsValue::from_js_object(obj);
        vm.regs[reg as usize] = val;
        val
    }

    #[test]
    fn reset_maybe_collect_collects_after_threshold() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        vm.promote_object(obj);
        assert!(!vm.gc_state.session_object_ptrs.is_empty());
        let tracked_before = vm.gc_state.session_object_ptrs.len();

        vm.regs[0] = JsValue::undefined();
        vm.regs[1] = JsValue::undefined();
        vm.reset();

        assert!(vm.gc_state.session_object_ptrs.len() <= tracked_before);
        assert_eq!(vm.gc_state.session_object_ptrs.len(), 0);
        assert_eq!(vm.gc_state.session_bytes_allocated, 0);
    }

    #[test]
    fn gc_stats_summary_includes_collection() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        vm.promote_object(obj);
        vm.regs[0] = JsValue::undefined();
        vm.maybe_collect_session_gc();
        let summary = vm.gc_state.session_gc.stats_summary();
        assert!(summary.contains("[GC] collection"));
    }

    #[test]
    fn moving_sweep_rewrites_global_root_edges() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        unsafe {
            (*obj).set_prop_at(0, JsValue::int(42));
        }
        let old_ptr = vm.promote_object(obj);
        let key = vm.kernel_core.perm_interner().intern("gcRoot").0;
        let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;
        unsafe {
            let global = &mut *global_ptr;
            vm.set_or_create_prop_value(global, key, JsValue::from_js_object(old_ptr));
        }

        vm.maybe_collect_session_gc();

        let global = vm.session.global_object();
        let pos = vm
            .kernel_core
            .shape_forge()
            .lookup_position(global.shape_id(), key)
            .expect("global slot");
        let new_value = global.get_prop_at(pos);
        assert!(new_value.is_object());
        assert!(!std::ptr::eq(new_value.as_js_object_ptr(), old_ptr));
        assert_eq!(unsafe { (*new_value.as_js_object_ptr()).get_prop_at(0) }, JsValue::int(42));
    }

    #[test]
    fn map_native_storage_is_not_a_normal_object_edge() {
        let mut vm = Vm::new();
        map_this(&mut vm, 1);
        let map_value = native_ok(map::map_constructor(&mut vm, &[1]));
        let map_obj = unsafe { &*map_value.as_js_object_ptr() };
        let native_ptr = map_obj.native_data() as *mut JsObject;

        assert!(map_obj.hash_props_vec().is_none());
        assert!(!map_obj.native_data().is_null());
        // Verify native storage pointer is not traced as a normal object edge.
        let mut stack = Vec::new();
        let mut live = HashSet::with_hasher(FxBuildHasher);
        let mut live_bigints = HashSet::with_hasher(FxBuildHasher);
        SessionGc::scan_edges_for_mark(map_obj, &vm, &mut stack, &mut live, &mut live_bigints);
        assert!(!stack.iter().any(|&ptr| std::ptr::eq(ptr, native_ptr)));
    }

    #[test]
    fn session_gc_traces_map_object_key_and_value() {
        let mut vm = vm_with_low_threshold();
        map_this(&mut vm, 3);
        let map_value = native_ok(map::map_constructor(&mut vm, &[3]));
        let key = JsValue::from_js_object(plain_object(&mut vm));
        let value = JsValue::from_js_object(plain_object(&mut vm));
        vm.regs[0] = map_value;
        vm.regs[1] = key;
        vm.regs[2] = value;
        native_ok(map::map_set(&mut vm, &[0, 1, 2]));

        let map_session = vm.promote_object(map_value.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(map_session);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(&mut vm);
        vm.gc_state.session_gc = gc;

        let live_map = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let edges = map::map_native_edges(live_map);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|value| vm.is_session_ptr(value.as_js_object_ptr())));
        assert_eq!(vm.gc_state.session_object_ptrs.len(), 3);
    }

    #[test]
    fn session_gc_traces_set_object_key() {
        let mut vm = vm_with_low_threshold();
        set_this(&mut vm, 2);
        let set_value = native_ok(set::set_constructor(&mut vm, &[2]));
        let key = JsValue::from_js_object(plain_object(&mut vm));
        vm.regs[0] = set_value;
        vm.regs[1] = key;
        native_ok(set::set_add(&mut vm, &[0, 1]));

        let set_session = vm.promote_object(set_value.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(set_session);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(&mut vm);
        vm.gc_state.session_gc = gc;

        let live_set = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let edges = set::set_native_edges(live_set);
        assert_eq!(edges.len(), 1);
        assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
        assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
    }

    #[test]
    fn session_gc_keeps_shared_array_buffer_alive_through_view_native_edges() {
        let mut vm = vm_with_low_threshold();
        let root = plain_object(&mut vm);

        vm.regs[1] = JsValue::int(8);
        let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));

        vm.regs[1] = buffer;
        let typed = native_ok(typed_array::int32array_constructor(&mut vm, &[0, 1]));

        vm.regs[1] = buffer;
        let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

        vm.regs[0] = view;
        vm.regs[1] = JsValue::int(0);
        vm.regs[2] = JsValue::int(42);
        vm.regs[3] = JsValue::bool(true);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

        unsafe {
            (*root).set_prop_at(0, typed);
            (*root).set_prop_at(1, view);
        }

        let root_session = vm.promote_object(root);
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(root_session);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(&mut vm);
        vm.gc_state.session_gc = gc;

        let live_root = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let live_typed = live_root.get_prop_at(0);
        let live_view = live_root.get_prop_at(1);
        let typed_obj = unsafe { &*live_typed.as_js_object_ptr() };
        let view_obj = unsafe { &*live_view.as_js_object_ptr() };
        let typed_edges = typed_array::typed_array_native_edges(typed_obj);
        let view_edges = data_view::data_view_native_edges(view_obj);

        assert_eq!(typed_edges.len(), 1);
        assert_eq!(view_edges.len(), 1);
        assert!(vm.is_session_ptr(typed_edges[0].as_js_object_ptr()));
        assert!(vm.is_session_ptr(view_edges[0].as_js_object_ptr()));
        assert!(std::ptr::eq(typed_edges[0].as_js_object_ptr(), view_edges[0].as_js_object_ptr()));

        vm.regs[0] = live_typed;
        vm.regs[1] = JsValue::int(0);
        assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(42));

        vm.regs[0] = live_view;
        vm.regs[1] = JsValue::int(4);
        vm.regs[2] = JsValue::int(7);
        vm.regs[3] = JsValue::bool(true);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

        vm.regs[0] = live_typed;
        vm.regs[1] = JsValue::int(1);
        assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(7));
    }

    #[test]
    fn session_gc_rewrites_buffer_retained_only_by_data_view_native_edge() {
        let mut vm = vm_with_low_threshold();
        vm.regs[1] = JsValue::int(8);
        let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));
        vm.regs[1] = buffer;
        let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

        vm.regs[0] = view;
        vm.regs[1] = JsValue::int(0);
        vm.regs[2] = JsValue::int(9);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2]));

        let view_session = vm.promote_object(view.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(view_session);
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(&mut vm);
        vm.gc_state.session_gc = gc;

        let live_view = vm.regs[0];
        let live_view_obj = unsafe { &*live_view.as_js_object_ptr() };
        let edges = data_view::data_view_native_edges(live_view_obj);
        assert_eq!(edges.len(), 1);
        assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
        assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);

        vm.regs[0] = live_view;
        vm.regs[1] = JsValue::int(0);
        assert_eq!(native_ok(data_view::data_view_get_int32(&mut vm, &[0, 1])), JsValue::int(9));
    }

    fn collect(vm: &mut Vm) {
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(vm);
        vm.gc_state.session_gc = gc;
    }

    #[test]
    fn session_string_collected_when_dead() {
        let mut vm = Vm::new();
        let dead = vm.new_string("dead-string");
        let dead_ptr = dead.as_string_ptr_mut();
        assert!(vm.gc_state.session_string_ptrs.contains(&dead_ptr));

        // 没有根引用 `dead`（它只作为值包装存在于 Rust 栈上）。
        collect(&mut vm);

        assert!(!vm.gc_state.session_string_ptrs.contains(&dead_ptr));
    }

    #[test]
    fn session_string_survives_when_in_register() {
        let mut vm = Vm::new();
        let live = vm.new_string("live-in-reg");
        let live_ptr = live.as_string_ptr_mut();
        vm.regs[0] = live;

        collect(&mut vm);

        assert!(vm.gc_state.session_string_ptrs.contains(&live_ptr));
        // 存活字符串永不搬移——寄存器仍指向同一 box。
        assert_eq!(vm.regs[0].as_string_ptr_mut(), live_ptr);
        assert_eq!(unsafe { (*live_ptr).as_str() }, "live-in-reg");
    }

    #[test]
    fn session_string_survives_via_object_property() {
        let mut vm = Vm::new();
        let obj = plain_object(&mut vm);
        let s = vm.new_string("prop-string");
        let s_ptr = s.as_string_ptr_mut();
        unsafe {
            (*obj).set_prop_at(0, s);
        }
        let obj_session = vm.promote_object(obj);

        // 该字符串仅通过存活对象的属性可达，不经过任何寄存器。
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(obj_session);

        collect(&mut vm);

        assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));
        assert_eq!(unsafe { (*s_ptr).as_str() }, "prop-string");
    }

    #[test]
    fn permanent_string_untouched_by_sweep() {
        let mut vm = Vm::new();
        let perm = vm.perm_string("perm");
        let perm_ptr = perm.as_string_ptr_mut();
        // 永久字符串位于 PermInterner，绝不在 session 集合中。
        assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
        vm.regs[0] = perm;

        collect(&mut vm);

        // 绝不被 session 清扫释放（仍可读，仍不在 session 集合中）。
        assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
        assert_eq!(unsafe { (*perm_ptr).as_str() }, "perm");
    }

    #[test]
    fn string_sweep_byte_accounting() {
        let mut vm = Vm::new();
        let dead = vm.new_string("0123456789");
        let _ = dead.as_string_ptr_mut();
        let expected = (size_of::<JsString>() + "0123456789".len()) as u64;

        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        let before = gc.total_bytes_freed;
        gc.collect(&mut vm);
        let after = gc.total_bytes_freed;
        vm.gc_state.session_gc = gc;

        assert!(after >= before + expected);
    }

    fn vm_with_threshold(bytes: usize) -> Vm {
        let mut cfg = KernelConfig::minimal();
        cfg.set_session_gc_threshold(bytes);
        let core = KernelCore::new(cfg);
        Vm::with_kernel_core(core)
    }

    // ── upvalue cell 独立堆分配：跨对象 sweep 存活 ─────────────────────────

    fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
        let allocator = Allocator::default();
        let program = oxide_parser::parse(&allocator, source).expect("parse");
        Compiler::new().compile(&program).expect("compile")
    }

    fn global_prop_opt(vm: &Vm, name: &str) -> Option<JsValue> {
        let global = vm.session.global_object();
        let si = vm.kernel_core().perm_interner().intern(name).0;
        vm.resolve_property(global, si)
    }

    /// 闭包捕获变量跨对象 sweep 存活：reset 触发完整收集（session_epoch 替换），
    /// 修复前 cell 分配于旧 arena、随 Bump drop 悬垂；修复后独立堆分配、
    /// 地址稳定，sweep 只重写 cell.value 中的对象引用，值跨搬移保留。
    #[test]
    fn closure_cell_survives_object_sweep() {
        let mut vm = vm_with_threshold(1);
        vm.run(&compile("var counter = 0; function inc() { return ++counter; } globalThis.inc = inc; 0"))
            .expect("run1");
        assert!(!vm.gc_state.session_cell_ptrs.borrow().is_empty(), "run1 应分配 upvalue cell");

        // reset 触发对象 sweep：存活对象搬到新 arena，旧 arena 释放。
        vm.reset();
        assert!(vm.session_gc_stats().total_collections > 0, "reset 应触发对象收集");

        // 跨搬移后从 global 重新取 inc 函数对象：upvalue cell 指针稳定、值保留。
        let inc_val = global_prop_opt(&vm, "inc").expect("inc 应挂在 global 上");
        let obj = unsafe { &*inc_val.as_js_object_ptr() };
        let cells = obj.upvalues_slice();
        assert_eq!(cells.len(), 1, "inc 应捕获 counter 一个 cell");
        let cell = unsafe { &*cells[0] };
        assert_eq!(cell.value, JsValue::int(0), "sweep 后 cell 值应保留为 counter 初值");
        assert!(cell.is_initialized(), "sweep 后 cell 初始化位应保留");
    }

    /// 私有字段类的 brand cell 跨对象 sweep 存活：`@@class_brand` upvalue cell
    /// 存类 brand 对象，sweep 后其值经 forwarding 重写为搬移后的新对象地址，
    /// cell 指针仍稳定（修复前 cell 悬垂、值被复用覆盖）。
    #[test]
    fn private_brand_cell_survives_object_sweep() {
        let mut vm = vm_with_threshold(1);
        vm.run(&compile(
            "class C { #x = 0; set(v){ this.#x = v; } get(){ return this.#x; } } \
             globalThis.c = new C(); globalThis.c.set(4); 0",
        ))
        .expect("run1");

        vm.reset();
        assert!(vm.session_gc_stats().total_collections > 0, "reset 应触发对象收集");

        // 实例 c 的方法 get（类原型上）捕获 @@class_brand cell：值须仍为对象。
        let c_val = global_prop_opt(&vm, "c").expect("c 应挂在 global 上");
        let c_obj = unsafe { &*c_val.as_js_object_ptr() };
        let proto_ptr = c_obj.proto().as_js_object_ptr();
        let get_val = vm
            .resolve_property(unsafe { &*proto_ptr }, vm.kernel_core().perm_interner().intern("get").0)
            .expect("类原型应有 get 方法");
        let get_obj = unsafe { &*get_val.as_js_object_ptr() };
        let cells = get_obj.upvalues_slice();
        assert!(!cells.is_empty(), "get 应捕获 @@class_brand cell");
        let brand = unsafe { &*cells[0] }.value;
        assert!(brand.is_object(), "sweep 后 brand cell 值应仍为 brand 对象");
    }

    /// full_reset 统一释放全部 cell box 且可重新分配：追踪表清空（无 double-free），
    /// 重置后新闭包正常建立新 cell。
    #[test]
    fn cells_freed_by_full_reset_and_reallocatable() {
        let mut vm = Vm::new();
        vm.run(&compile("var x = 1; function f() { return x; } globalThis.f = f; f()"))
            .expect("run1");
        assert!(!vm.gc_state.session_cell_ptrs.borrow().is_empty());

        vm.full_reset();
        assert!(vm.gc_state.session_cell_ptrs.borrow().is_empty(), "full_reset 应释放全部 cell");

        let result = vm.run(&compile("var y = 2; function g() { return y; } g()")).expect("run2");
        assert_eq!(format!("{result}"), "2", "重置后新 cell 应正常分配与读取");
    }

    #[test]
    fn allocation_does_not_trigger_gc_before_safe_point() {
        let mut vm = vm_with_threshold(1);
        // 分配点不触发回收：触发已移到 dispatch 指令边界，局部持有跨分配点安全。
        let seed = vm.new_string_owned("seed".repeat(16));
        let seed_ptr = seed.as_string_ptr_mut();
        let returned = vm.new_string_owned("returned".repeat(16));

        assert_eq!(vm.gc_state.session_gc.total_collections, 0);
        assert!(vm.gc_state.session_string_ptrs.contains(&seed_ptr));
        assert!(vm.gc_state.session_string_ptrs.contains(&returned.as_string_ptr_mut()));

        // 显式触发后：无根引用的 seed 被回收，入根的 returned 存活（安全点语义）。
        vm.regs[0] = returned;
        vm.maybe_collect_session_strings();
        assert!(vm.gc_state.session_gc.total_collections >= 1);
        assert!(!vm.gc_state.session_string_ptrs.contains(&seed_ptr));
        assert!(vm.gc_state.session_string_ptrs.contains(&returned.as_string_ptr_mut()));
    }

    #[test]
    fn strings_only_collection_preserves_all_root_kinds() {
        // 阈值 1：每次 new_string_owned 分配前自动触发回收，逐步验证各类根的保护。
        let mut vm = vm_with_threshold(1);
        // 寄存器根：直接持有 session 串。
        let reg_str = vm.new_string_owned("reg-root".repeat(16));
        vm.regs[0] = reg_str;
        // 存活对象属性根：session 对象经 promote 后持有 session 串（分配即触发回收，
        // reg_str 仍在寄存器，prop_str 挂到对象后才被下次回收看到）。
        let obj = plain_object(&mut vm);
        let prop_str = vm.new_string_owned("prop-root".repeat(16));
        unsafe {
            (*obj).set_prop_at(0, prop_str);
        }
        let obj_session = vm.promote_object(obj);
        vm.regs[1] = JsValue::from_js_object(obj_session);
        // 非 session 根对象属性：epoch 根对象（在寄存器）持有 session 串，mark 走
        // 非 session 根扫描路径保护。
        let epoch_obj = plain_object(&mut vm);
        let epoch_str = vm.new_string_owned("epoch-root".repeat(16));
        unsafe {
            (*epoch_obj).set_prop_at(0, epoch_str);
        }
        vm.regs[2] = JsValue::from_js_object(epoch_obj);
        // 死串：无任何根引用，最后手动触发一轮回收它。
        let dead = vm.new_string_owned("dead".repeat(16));
        let dead_ptr = dead.as_string_ptr_mut();

        vm.maybe_collect_session_strings();

        assert!(vm.gc_state.session_string_ptrs.contains(&reg_str.as_string_ptr_mut()));
        assert!(vm.gc_state.session_string_ptrs.contains(&prop_str.as_string_ptr_mut()));
        assert!(vm.gc_state.session_string_ptrs.contains(&epoch_str.as_string_ptr_mut()));
        assert!(!vm.gc_state.session_string_ptrs.contains(&dead_ptr));
        // 存活串内容可读，地址稳定。
        assert_eq!(unsafe { (*reg_str.as_string_ptr_mut()).as_str() }, "reg-root".repeat(16));
        assert_eq!(unsafe { (*prop_str.as_string_ptr_mut()).as_str() }, "prop-root".repeat(16));
        assert_eq!(unsafe { (*epoch_str.as_string_ptr_mut()).as_str() }, "epoch-root".repeat(16));
    }

    #[test]
    fn strings_only_collection_does_not_move_objects() {
        let mut vm = vm_with_threshold(1);
        let obj = plain_object(&mut vm);
        let obj_session = vm.promote_object(obj);
        vm.regs[0] = JsValue::from_js_object(obj_session);
        let before: Vec<_> = vm.gc_state.session_object_ptrs.clone();

        let s = vm.new_string_owned("x".repeat(64));
        vm.regs[1] = s;
        // 分配不自动触发（安全点语义），显式触发一轮验证对象不搬移。
        vm.maybe_collect_session_strings();

        // 对象指针逐一相同：不搬移、不重写根。
        assert_eq!(vm.gc_state.session_object_ptrs, before);
        assert_eq!(vm.regs[0].as_js_object_ptr(), obj_session);
        assert_eq!(vm.session_object_count(), 1);
    }

    #[test]
    fn multiple_strings_only_cycles_keep_objects_alive() {
        let mut vm = vm_with_threshold(1);
        let obj = plain_object(&mut vm);
        let obj_session = vm.promote_object(obj);
        let live = vm.new_string_owned("keep".repeat(32));
        unsafe {
            (*obj_session).set_prop_at(0, live);
        }
        vm.regs[0] = JsValue::from_js_object(obj_session);

        // 连续多轮触发字符串回收：对象 mark 残留位被显式清理，对象与挂载串持续存活。
        for _ in 0..5 {
            for _ in 0..8 {
                let _ = vm.new_string_owned("dead".repeat(64));
            }
            // 分配不自动触发，每轮显式触发一次回收。
            vm.maybe_collect_session_strings();
            assert!(vm.gc_state.session_object_ptrs.contains(&obj_session));
            assert_eq!(unsafe { (*obj_session).get_prop_at(0) }, live);
        }
        assert!(vm.gc_state.session_string_ptrs.contains(&live.as_string_ptr_mut()));
    }

    #[test]
    fn strings_only_then_full_collect_keeps_object_strings_live() {
        // strings-only 收集后接完整收集（reset 路径）：strings-only 残留的 mark 位
        // 不得使完整收集的 mark DFS 短路漏标，存活对象属性中的串须跨收集存活。
        let mut vm = vm_with_threshold(1);
        let obj = plain_object(&mut vm);
        let s = vm.new_string_owned("kept".repeat(8));
        let s_ptr = s.as_string_ptr_mut();
        unsafe {
            (*obj).set_prop_at(0, s);
        }
        let obj_session = vm.promote_object(obj);
        vm.regs[0] = JsValue::from_js_object(obj_session);

        // 第一轮 strings-only：对象被 mark 置位，修复前该位残留至完整收集。
        vm.maybe_collect_session_strings();
        assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));

        // 完整收集（reset 的 maybe_collect 路径）：对象搬移、字符串地址稳定。
        let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
        gc.collect(&mut vm);
        vm.gc_state.session_gc = gc;

        // 存活对象经重写仍可达其串：串未被误释放，内容可读。
        assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));
        assert_eq!(unsafe { (*s_ptr).as_str() }, "kept".repeat(8));
        assert_eq!(unsafe { (*vm.regs[0].as_js_object_ptr()).get_prop_at(0) }, s);
    }

    #[test]
    fn strings_only_collection_byte_accounting_matches_survivors() {
        let mut vm = vm_with_threshold(1);
        let obj = plain_object(&mut vm);
        let obj_session = vm.promote_object(obj);
        vm.regs[0] = JsValue::from_js_object(obj_session);
        let live = vm.new_string_owned("live".repeat(8));
        let live_ptr = live.as_string_ptr_mut();
        vm.regs[1] = live;
        let dead = vm.new_string_owned("dead".repeat(8));
        let _ = dead.as_string_ptr_mut();

        vm.maybe_collect_session_strings();

        // 账目 = 对象头 + 存活串（size_of::<JsString>() + len），死串不再计入。
        let expected = size_of::<JsObject>() + (size_of::<JsString>() + unsafe { (*live_ptr).len() });
        assert_eq!(vm.gc_state.session_bytes_allocated, expected);
        assert!(!vm.gc_state.session_string_ptrs.contains(&dead.as_string_ptr_mut()));
    }

    #[test]
    fn runtime_collection_below_threshold_is_noop() {
        let mut vm = Vm::new();
        let s = vm.new_string_owned("small".repeat(4));
        vm.regs[0] = s;

        assert_eq!(vm.gc_state.session_gc.total_collections, 0);
        assert!(vm.gc_state.session_string_ptrs.contains(&s.as_string_ptr_mut()));
    }

    // ── rope（Cons）GC 传播闭包 ──

    /// 构造 `left + right` 的 Cons 节点（返回 (父, 左, 右) 三指针）。
    fn make_cons_pair(vm: &mut Vm, l: &str, r: &str) -> (JsValue, *mut JsString, *mut JsString) {
        let left = vm.new_string(l);
        let right = vm.new_string(r);
        let parent = vm.new_cons_string(left, right);
        (parent, left.as_string_ptr_mut(), right.as_string_ptr_mut())
    }

    #[test]
    fn rope_survives_collection_with_children_propagated() {
        let mut vm = Vm::new();
        let (parent, l_ptr, r_ptr) = make_cons_pair(&mut vm, "left-part", "right-part");
        vm.regs[0] = parent;

        collect(&mut vm);

        // 父（寄存器根）+ 子节点（经传播闭包）全部存活，内容可读。
        let parent_ptr = parent.as_string_ptr_mut();
        assert!(vm.gc_state.session_string_ptrs.contains(&parent_ptr));
        assert!(vm.gc_state.session_string_ptrs.contains(&l_ptr));
        assert!(vm.gc_state.session_string_ptrs.contains(&r_ptr));
        assert_eq!(unsafe { (*parent_ptr).as_str() }, "left-partright-part");
    }

    #[test]
    fn rope_children_swept_when_parent_dead() {
        let mut vm = Vm::new();
        let (parent, l_ptr, r_ptr) = make_cons_pair(&mut vm, "left", "right");
        let parent_ptr = parent.as_string_ptr_mut();
        assert!(vm.gc_state.session_string_ptrs.contains(&parent_ptr));

        // 父与子均无根引用 → 整树回收。
        collect(&mut vm);

        assert!(!vm.gc_state.session_string_ptrs.contains(&parent_ptr));
        assert!(!vm.gc_state.session_string_ptrs.contains(&l_ptr));
        assert!(!vm.gc_state.session_string_ptrs.contains(&r_ptr));
    }

    #[test]
    fn rope_product_freed_with_parent() {
        let mut vm = Vm::new();
        let (parent, _, _) = make_cons_pair(&mut vm, "left-part", "right-part");
        let parent_ptr = parent.as_string_ptr_mut();
        // 触发扁平化：产物发布到 flat_cache。
        assert_eq!(unsafe { (*parent_ptr).flat_str() }, "left-partright-part");
        let flat_ptr = unsafe { (*parent_ptr).flat_cache_ptr() };
        assert!(!flat_ptr.is_null());

        collect(&mut vm);

        // 父死 → 连带释放产物（不 double-free、不泄漏；产物从不进 session 表）。
        let flat_mut = flat_ptr as *mut JsString;
        assert!(!vm.gc_state.session_string_ptrs.contains(&parent_ptr));
        assert!(!vm.gc_state.session_string_ptrs.contains(&flat_mut));
    }

    #[test]
    fn rope_deep_chain_mark_iterative() {
        let mut vm = Vm::new();
        // 1024 层左倾链：mark 传播用显式栈，不爆栈、不遗漏子节点。
        let mut chain = vm.new_string("root");
        for _ in 0..1024 {
            let leaf = vm.new_string("x");
            chain = vm.new_cons_string(chain, leaf);
        }
        vm.regs[0] = chain;
        let chain_ptr = chain.as_string_ptr_mut();

        collect(&mut vm);

        assert!(vm.gc_state.session_string_ptrs.contains(&chain_ptr));
        assert_eq!(unsafe { (*chain_ptr).as_str() }, format!("root{}", "x".repeat(1024)));
    }

    #[test]
    fn rope_perm_child_untouched_by_sweep() {
        let mut vm = Vm::new();
        let perm = vm.perm_string("perm-leaf");
        let perm_ptr = perm.as_string_ptr_mut();
        let session = vm.new_string("session-leaf");
        let session_ptr = session.as_string_ptr_mut();
        let parent = vm.new_cons_string(perm, session);
        vm.regs[0] = parent;

        collect(&mut vm);

        // perm 子节点永不释放（不在 session 表）；session 子节点随父存活。
        assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
        assert!(vm.gc_state.session_string_ptrs.contains(&session_ptr));
        assert_eq!(unsafe { (*parent.as_string_ptr_mut()).as_str() }, "perm-leafsession-leaf");
    }

    #[test]
    fn full_reset_frees_rope_and_product() {
        let mut vm = Vm::new();
        let (parent, _, _) = make_cons_pair(&mut vm, "left-part", "right-part");
        // 触发扁平化（产物挂在父上）。
        assert_eq!(unsafe { (*parent.as_string_ptr_mut()).flat_str() }, "left-partright-part");
        vm.regs[0] = parent;

        // full_reset 清空全部 session 字符串（连带产物）：无泄漏、无 double-free。
        vm.full_reset();
        assert!(vm.gc_state.session_string_ptrs.is_empty());
    }
}
