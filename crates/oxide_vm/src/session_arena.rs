use std::collections::HashMap;

use crate::vm_debug;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::vm::Vm;
use oxide_builtins::{array_buffer, data_view, disposable_stack, map, module, regexp, set, typed_array};

impl Vm {
    /// 单对象晋升测试入口：取/清共享转发表后调 `promote_object_inner`。
    /// 生产路径（边界修复/执行期晋升档/根晋升）各自持表管理，不经过此包装。
    #[cfg(test)]
    pub(crate) fn promote_object(&mut self, src: *mut JsObject) -> *mut JsObject {
        vm_debug!("promote_object: src={:p}", src);
        let mut forwarding = std::mem::take(&mut self.gc_state.forwarding);
        let result = self.promote_object_inner(src, &mut forwarding);
        forwarding.clear();
        self.gc_state.forwarding = forwarding;
        result
    }

    pub(crate) fn promote_object_inner(
        &mut self, src: *mut JsObject, forwarding: &mut HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
    ) -> *mut JsObject {
        vm_debug!("promote_object_inner: src={:p}", src);
        if src.is_null() {
            return src;
        }
        let src_ref = unsafe { &*src };
        if src_ref.is_session_epoch() || !src_ref.is_epoch() {
            return src;
        }
        if let Some(dst) = forwarding.get(&src).copied() {
            return dst;
        }

        let clone = src_ref.clone_for_session_epoch();
        let dst = self.gc_state.session_epoch.alloc(clone) as *mut JsObject;
        forwarding.insert(src, dst);
        self.gc_state.session_object_ptrs.push(dst);

        let dst_ref = unsafe { &mut *dst };
        dst_ref.rewrite_object_values(|value| self.promote_value_if_epoch_object(value, forwarding));
        if src_ref.is_map() {
            map::clone_map_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_set() {
            set::clone_set_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_disposable_stack_obj() || src_ref.is_async_disposable_stack_obj() {
            // 资源栈状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            disposable_stack::clone_dispose_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_typed_array_obj() {
            typed_array::clone_typed_array_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_data_view_obj() {
            data_view::clone_data_view_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_array_buffer_obj() {
            // 字节缓冲深拷贝到新对象：源 Vec 随 epoch 释放，互不共享。
            array_buffer::clone_array_buffer_native(src_ref, dst_ref);
        } else if src_ref.is_shared_array_buffer_obj() {
            // 字节缓冲深拷贝到新对象：源 Vec 随 epoch 释放，互不共享。
            array_buffer::clone_shared_array_buffer_native(src_ref, dst_ref);
        } else if src_ref.is_generator_obj() {
            // 生成器状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            crate::generator::clone_generator_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_promise_obj() {
            // Promise 状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享；
            // 反应迁入新克隆并接结算链（原件/旧克隆结算沿链传导到最新克隆）。
            crate::promise::clone_promise_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
            crate::promise::migrate_settlement_to_newest_clone(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_async_obj() {
            // 异步状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            crate::async_func::clone_async_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_async_generator_obj() {
            // 异步生成器状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            crate::async_generator::clone_async_generator_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.holds_compiled_regex() {
            // 已编译正则是 Box 深拷贝到新对象：源盒随 epoch 释放，互不共享。
            regexp::clone_regexp_native(src_ref, dst_ref);
        } else if src_ref.is_module_namespace() {
            // 条目表深拷贝到新对象并改写值边：`clone_for_session_epoch` 只浅拷贝
            // native_data 指针，共享同一表会在释放时双放。
            module::clone_module_ns_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        }
        // 账目计入：对象头 + 堆数据（属性/元素/meta Vec + native 状态盒），用 clone 后的 dst 核算。
        self.gc_state.session_bytes_allocated +=
            std::mem::size_of::<JsObject>() + crate::session_gc::SessionGc::object_heap_data_bytes(dst_ref) as usize;
        dst
    }

    /// 克隆改写与 reset 边界晋升共用的单引用改写：epoch 对象克隆晋升进
    /// session（forwarding 去重共享与环）；其余值原样保留。
    pub(crate) fn promote_value_if_epoch_object(
        &mut self, value: JsValue, forwarding: &mut HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
    ) -> JsValue {
        if !value.is_object() {
            return value;
        }
        let ptr = value.as_js_object_ptr();
        if ptr.is_null() {
            return value;
        }
        // SAFETY: 执行核心产出的对象值，指针在 session 生命周期内有效。
        if unsafe { &*ptr }.is_epoch() {
            return JsValue::from_js_object(self.promote_object_inner(ptr, forwarding));
        }
        value
    }

    /// epoch 边界前的 session 原地晋升：遍历全部 session 对象，把仍指向 epoch
    /// 对象的引用克隆进 session（JS 边 + native 状态盒），消除 epoch 重置后的
    /// 悬垂指针。
    ///
    /// 覆盖三类绕过写屏障的引用来源：函数对象直接 session 分配后捕获的
    /// `captured_this`/`home_object`/upvalue cell/属性值（SET_HOME_OBJECT 与
    /// 函数目标豁免均直落 epoch 值）；Map/Set 等原生盒按键值直插 epoch 值；
    /// global 对象的属性值（逃逸写直通后 epoch 值直落 global 槽，global 不入
    /// session 对象表，由 `rewrite_session_epoch_refs` 同趟改写）。
    /// `rewrite_object_values` 覆盖元素/meta/属性/proto/captured_this/home_object/
    /// cell 值；克隆子树经 forwarding 去重，环与共享引用各克隆一次。
    ///
    /// # 注意事项
    /// - 须在执行外的安全点调用（无在途 builtin 局部裸指针、dispatch 未重入）：
    ///   本方法原地改写 session 对象的引用字段，执行期调用将使 builtin 局部
    ///   裸指针失效（与 `collect_session_gc` 同前提）。
    /// - session 对象表为空时 global 的 epoch 子引用仍须修复，不做空表早退。
    pub fn promote_session_epoch_refs(&mut self) {
        let objects = std::mem::take(&mut self.gc_state.session_object_ptrs);
        let mut forwarding = std::mem::take(&mut self.gc_state.forwarding);
        self.rewrite_session_epoch_refs(&objects, &mut forwarding);
        forwarding.clear();
        self.gc_state.forwarding = forwarding;
        // 改写期新克隆已随晋升推入 gc_state 侧的表（此刻仅含克隆体），把取出的
        // 旧对象按原相对序拼回同一表，表 = 克隆体 + 旧对象，各恰登记一次。
        self.gc_state.session_object_ptrs.extend(objects);
    }

    /// 按调用方给定的转发表把对象列表的 epoch 子引用就地改写：未在上游晋升过的
    /// epoch 子对象经转发表克隆进 session（递归去重共享与环），已是 session/
    /// 非 epoch 的值原样保留。JS 边与 native 状态盒走同一改写闭包。
    ///
    /// global 对象不入 session 对象表（P 根，不在 arena 内）：其 epoch 子引用
    /// 与列表同趟改写，无 native 盒只覆盖 JS 边。
    ///
    /// # 注意事项
    /// - 调用方持有转发表期间不得让其它晋升路径改动 `gc_state.forwarding`；
    ///   晋升新克隆推入的对象表由调用方决定何时拼回。
    /// - 死对象（不可达）的 epoch 子引用改写会把死对象一并克隆进 session——
    ///   克隆体无根，下一轮收集按死对象出表，不泄漏也不悬垂。
    pub(crate) fn rewrite_session_epoch_refs(
        &mut self, objects: &[*mut JsObject], forwarding: &mut HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
    ) {
        for &ptr in objects {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session_epoch.alloc，arena 存活期内有效。
            unsafe {
                let obj = &mut *ptr;
                obj.rewrite_object_values(|value| self.promote_value_if_epoch_object(value, forwarding));
                if obj.is_map() {
                    map::rewrite_map_native(obj, |value| self.promote_value_if_epoch_object(value, forwarding));
                } else if obj.is_set() {
                    set::rewrite_set_native(obj, |value| self.promote_value_if_epoch_object(value, forwarding));
                } else if obj.is_disposable_stack_obj() || obj.is_async_disposable_stack_obj() {
                    disposable_stack::rewrite_dispose_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_typed_array_obj() {
                    typed_array::rewrite_typed_array_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_data_view_obj() {
                    data_view::rewrite_data_view_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_generator_obj() {
                    crate::generator::rewrite_generator_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_promise_obj() {
                    crate::promise::rewrite_promise_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_async_obj() {
                    crate::async_func::rewrite_async_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_async_generator_obj() {
                    crate::async_generator::rewrite_async_generator_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                } else if obj.is_module_namespace() {
                    module::rewrite_module_ns_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, forwarding)
                    });
                }
            }
        }
        // global 对象不入 session 对象表：其 epoch 子引用（逃逸写直通留存、
        // 函数目标豁免、native 盒直插）与列表同趟修复，免 epoch 释放后悬垂。
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        // SAFETY: global 归本 session 所有，安全点内指针有效。
        unsafe {
            let obj = &mut *global_ptr;
            obj.rewrite_object_values(|value| self.promote_value_if_epoch_object(value, forwarding));
        }
    }

    /// 把根直接持有的 epoch 对象（顶层 var 寄存器、挂起句柄等）晋升进 session，
    /// 并把根引用改写到克隆体。
    ///
    /// # 副作用
    /// - 每个根 epoch 对象深克隆进 session（含 native 状态盒，子引用递归晋升），
    ///   账目计入克隆字节；根引用按转发表重写。
    ///
    /// # 注意事项
    /// - 须在执行外的安全点调用（无在途 builtin 局部裸指针、dispatch 未重入）：
    ///   本方法改写全部根寄存器槽，执行期调用将使 builtin 局部裸指针失效
    ///   （与 `collect_session_gc` 同前提）。
    /// - 供 workload 后观测点（基准留存测量）使用：本方法之后存活集完全对
    ///   session 可见，后续完整 GC 的留存账目不再漏"仅驻留 epoch arena 的对象"。
    /// - 与 `promote_session_epoch_refs` 互补：本方法处理根直接持有的 epoch 对象，
    ///   后者处理 session 对象持有的 epoch 子引用（闭包捕获等绕过写屏障的来源）。
    pub fn promote_rooted_epoch_objects(&mut self) {
        let mut epoch_roots = Vec::new();
        self.for_each_root(|value| {
            if value.is_object() {
                let ptr = value.as_js_object_ptr();
                if !ptr.is_null() {
                    // SAFETY: 根上的对象值均为 VM 自有存活 JsObject。
                    if unsafe { &*ptr }.is_epoch() {
                        epoch_roots.push(ptr);
                    }
                }
            }
        });
        let mut forwarding = std::mem::take(&mut self.gc_state.forwarding);
        for ptr in epoch_roots {
            self.promote_object_inner(ptr, &mut forwarding);
        }
        crate::session_gc::rewrite_vm_roots(self, &forwarding);
        forwarding.clear();
        self.gc_state.forwarding = forwarding;
    }

    /// 逃逸写屏障：写向全局/session 目标的对象值原样直通。
    ///
    /// session 对象持有 epoch 子引用是 GC 设计的一等存活态：epoch 边界 reset
    /// 经 `promote_session_epoch_refs` 统一克隆晋升（转发表去重共享与环），
    /// 执行期收集晋升档按同一转发表改写根与存活 session 对象子引用，两条
    /// 路径覆盖全部跨界持有面；builtin 调用期内 epoch 不重置，帧局部裸指针
    /// 全程有效。此处若克隆，同一逻辑对象分裂为原件与克隆两份，经克隆的写
    /// 对方经原件的读不可见（builtin 帧指针与逃逸目标双引用并存时即暴露）。
    pub(crate) fn promote_if_needed_for_write_ptr(&mut self, _target_ptr: *mut JsObject, value: JsValue) -> JsValue {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_types::object::{PropAttributes, PropMetaEntry};

    fn plain_object(vm: &mut Vm) -> *mut JsObject {
        let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let ptr = vm
            .epoch
            .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*ptr).set_is_epoch(true) };
        ptr
    }

    fn is_epoch_object(_vm: &Vm, value: JsValue) -> bool {
        value.is_object() && unsafe { (&*value.as_js_object_ptr()).is_epoch() }
    }

    #[test]
    fn session_arena_promotion_preserves_cycle_identity() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(root));
        }

        let promoted = vm.promote_object(root);
        let promoted_obj = unsafe { &*promoted };

        assert!(promoted_obj.is_session_epoch());
        assert!(std::ptr::eq(promoted_obj.get_prop_at(0).as_js_object_ptr(), promoted));
    }

    #[test]
    fn session_arena_promotion_preserves_duplicate_child_reference() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let child = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
            (*root).set_prop_at(1, JsValue::from_js_object(child));
        }

        let promoted = vm.promote_object(root);
        let promoted_obj = unsafe { &*promoted };
        let left = promoted_obj.get_prop_at(0).as_js_object_ptr();
        let right = promoted_obj.get_prop_at(1).as_js_object_ptr();

        assert!(std::ptr::eq(left, right));
        assert!(!std::ptr::eq(left, child));
        assert!(unsafe { (&*left).is_session_epoch() });
    }

    #[test]
    fn session_arena_captured_and_home_object_links_promote() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let proto = plain_object(&mut vm);
        let captured = plain_object(&mut vm);
        let home = plain_object(&mut vm);
        let getter = plain_object(&mut vm);
        let setter = plain_object(&mut vm);
        unsafe {
            (*root).set_proto(JsValue::from_js_object(proto)).expect("proto");
            (*root).set_captured_this(JsValue::from_js_object(captured));
            (*root).set_home_object(JsValue::from_js_object(home));
            (*root).set_prop_at(0, JsValue::undefined());
            (*root).set_accessor_meta(
                0,
                JsValue::from_js_object(getter),
                JsValue::from_js_object(setter),
                PropAttributes::DEFAULT_DATA,
            );
        }

        let promoted = vm.promote_object(root);
        let promoted_obj = unsafe { &*promoted };
        let meta = promoted_obj.prop_meta_at(0).expect("meta");

        assert!(!is_epoch_object(&vm, promoted_obj.proto()));
        assert!(!is_epoch_object(&vm, promoted_obj.captured_this()));
        assert!(!is_epoch_object(&vm, promoted_obj.home_object()));
        assert!(!is_epoch_object(&vm, meta.get));
        assert!(!is_epoch_object(&vm, meta.set));
    }

    #[test]
    fn session_arena_promotes_array_elements_and_element_meta() {
        let mut vm = Vm::new();
        let elem_child = plain_object(&mut vm);
        let meta_getter = plain_object(&mut vm);
        let array_proto = vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let arr = vm.epoch.alloc(JsObject::new_array(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(array_proto),
            2,
            vm.epoch.bump(),
        ));
        // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
        unsafe { (*arr).set_is_epoch(true) };
        unsafe {
            (*arr).set_prop_at(0, JsValue::from_js_object(elem_child));
            (*arr).set_accessor_meta(
                1,
                JsValue::from_js_object(meta_getter),
                JsValue::undefined(),
                PropAttributes::DEFAULT_DATA,
            );
            (*arr).set_prop_shape(0, JsValue::int(99));
        }

        let promoted = vm.promote_object(arr);
        let promoted_arr = unsafe { &*promoted };

        assert!(promoted_arr.is_session_epoch());
        // 数组元素（含访问器 get）与命名属性随 clone + rewrite 迁移到 session。
        assert!(!is_epoch_object(&vm, promoted_arr.get_prop_at(0)));
        let meta = promoted_arr.prop_meta_at(1).expect("element accessor meta");
        assert!(!is_epoch_object(&vm, meta.get));
        assert_eq!(promoted_arr.get_prop_shape(0), JsValue::int(99));
        // 元素计数与长度不因 promotion 改变。
        assert_eq!(promoted_arr.prop_count(), 2);
    }

    #[test]
    fn session_arena_barrier_global_root_write_returns_original() {
        let mut vm = Vm::new();
        let value = JsValue::from_js_object(plain_object(&mut vm));
        let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;

        let promoted = vm.promote_if_needed_for_write_ptr(global_ptr, value);

        assert_eq!(promoted, value);
        assert!(is_epoch_object(&vm, promoted), "epoch 子引用应直通留存，由边界/晋升档统一修复");
    }

    #[test]
    fn session_arena_barrier_session_target_write_returns_original() {
        let mut vm = Vm::new();
        let target_epoch = plain_object(&mut vm);
        let target = vm.promote_object(target_epoch);
        let value = JsValue::from_js_object(plain_object(&mut vm));

        let promoted = vm.promote_if_needed_for_write_ptr(target, value);

        assert_eq!(promoted, value);
        assert!(is_epoch_object(&vm, promoted), "session 目标上的 epoch 值应直通留存");
    }

    #[test]
    fn session_arena_barrier_leaves_non_escape_target_write_unchanged() {
        let mut vm = Vm::new();
        let target = plain_object(&mut vm);
        let value = JsValue::from_js_object(plain_object(&mut vm));

        let unchanged = vm.promote_if_needed_for_write_ptr(target, value);

        assert_eq!(unchanged, value);
    }

    #[test]
    fn session_arena_promotion_rewrites_accessor_metadata_values() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let getter = plain_object(&mut vm);
        let setter = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::undefined());
            (*root).set_accessor_meta(
                0,
                JsValue::from_js_object(getter),
                JsValue::from_js_object(setter),
                PropAttributes::DEFAULT_DATA,
            );
        }

        let promoted = vm.promote_object(root);
        let meta: PropMetaEntry = unsafe { &*promoted }.prop_meta_at(0).expect("meta");

        assert!(!is_epoch_object(&vm, meta.get));
        assert!(!is_epoch_object(&vm, meta.set));
    }

    #[test]
    fn promote_clears_forwarding_map() {
        let mut vm = Vm::new();
        let first = plain_object(&mut vm);
        let promoted = vm.promote_object(first);
        assert!(!promoted.is_null());
        // 共享 forwarding 表必须在每次 promote 后清空，使后续 promote（或 GC 清扫）
        // 永不观察到过期的 old->new 映射。
        assert!(vm.gc_state.forwarding.is_empty());

        let second = plain_object(&mut vm);
        let promoted2 = vm.promote_object(second);
        assert!(!promoted2.is_null());
        assert!(vm.gc_state.forwarding.is_empty());
    }
}
