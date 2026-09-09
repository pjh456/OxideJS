use std::collections::HashMap;

use crate::vm_debug;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::vm::Vm;
use oxide_builtins::{array_buffer, data_view, disposable_stack, map, regexp, set, typed_array};

impl Vm {
    pub(crate) fn is_session_escape_root_ptr(&self, target_ptr: *mut JsObject) -> bool {
        if target_ptr.is_null() {
            return false;
        }
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        std::ptr::eq(target_ptr, global_ptr) || unsafe { (&*target_ptr).is_session_epoch() }
    }

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
        } else if src_ref.is_generator_obj() {
            // 生成器状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            crate::generator::clone_generator_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_promise_obj() {
            // Promise 状态盒深拷贝到新对象：源盒随 epoch 释放，互不共享。
            crate::promise::clone_promise_native_with_rewrite(src_ref, dst_ref, |value| {
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
        } else if src_ref.is_regexp_obj() {
            // 已编译正则是 Box 深拷贝到新对象：源盒随 epoch 释放，互不共享。
            regexp::clone_regexp_native(src_ref, dst_ref);
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
    /// 覆盖两类绕过写屏障的引用来源：函数对象直接 session 分配后捕获的
    /// `captured_this`/`home_object`/upvalue cell/属性值（SET_HOME_OBJECT 与
    /// 函数目标豁免均直落 epoch 值）；Map/Set 等原生盒按键值直插 epoch 值。
    /// `rewrite_object_values` 覆盖元素/meta/属性/proto/captured_this/home_object/
    /// cell 值；克隆子树经 forwarding 去重，环与共享引用各克隆一次。
    pub(crate) fn promote_session_epoch_refs(&mut self) {
        let objects = std::mem::take(&mut self.gc_state.session_object_ptrs);
        if objects.is_empty() {
            return;
        }
        let mut forwarding = std::mem::take(&mut self.gc_state.forwarding);
        for &ptr in &objects {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session_epoch.alloc，arena 存活期内有效。
            unsafe {
                let obj = &mut *ptr;
                obj.rewrite_object_values(|value| self.promote_value_if_epoch_object(value, &mut forwarding));
                if obj.is_map() {
                    map::rewrite_map_native(obj, |value| self.promote_value_if_epoch_object(value, &mut forwarding));
                } else if obj.is_set() {
                    set::rewrite_set_native(obj, |value| self.promote_value_if_epoch_object(value, &mut forwarding));
                } else if obj.is_disposable_stack_obj() || obj.is_async_disposable_stack_obj() {
                    disposable_stack::rewrite_dispose_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_typed_array_obj() {
                    typed_array::rewrite_typed_array_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_data_view_obj() {
                    data_view::rewrite_data_view_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_generator_obj() {
                    crate::generator::rewrite_generator_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_promise_obj() {
                    crate::promise::rewrite_promise_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_async_obj() {
                    crate::async_func::rewrite_async_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                } else if obj.is_async_generator_obj() {
                    crate::async_generator::rewrite_async_generator_native(obj, |value| {
                        self.promote_value_if_epoch_object(value, &mut forwarding)
                    });
                }
            }
        }
        forwarding.clear();
        self.gc_state.forwarding = forwarding;
        // 晋升过程新克隆的对象已推入 gc_state 侧的表，拼回旧表保持原序在前。
        self.gc_state.session_object_ptrs.extend(objects);
    }

    /// 逃逸写屏障：写向全局/session 对象的对象值不指向 epoch，否则 epoch
    /// 重置后悬垂。session 函数目标豁免晋升——克隆会令写入侧与字节码持有的
    /// 原始对象分裂（类构造器在原型上续建方法会落到非克隆体）；函数持有的
    /// epoch 子引用由 `promote_session_epoch_refs` 在 epoch 边界统一修复。
    pub(crate) fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue {
        if !value.is_object() || !self.is_session_escape_root_ptr(target_ptr) {
            return value;
        }
        let value_ptr = value.as_js_object_ptr();
        if value_ptr.is_null() {
            return value;
        }
        // SAFETY: 执行核心产出的对象值，指针在 session 生命周期内有效。
        if unsafe { &*target_ptr }.is_function() {
            return value;
        }
        if unsafe { &*value_ptr }.is_epoch() {
            return JsValue::from_js_object(self.promote_object(value_ptr));
        }
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
    fn session_arena_barrier_promotes_global_root_write() {
        let mut vm = Vm::new();
        let value = JsValue::from_js_object(plain_object(&mut vm));
        let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;

        let promoted = vm.promote_if_needed_for_write_ptr(global_ptr, value);

        assert!(promoted.is_object());
        assert!(!is_epoch_object(&vm, promoted));
        assert!(unsafe { (&*promoted.as_js_object_ptr()).is_session_epoch() });
    }

    #[test]
    fn session_arena_barrier_promotes_already_session_target_write() {
        let mut vm = Vm::new();
        let target_epoch = plain_object(&mut vm);
        let target = vm.promote_object(target_epoch);
        let value = JsValue::from_js_object(plain_object(&mut vm));

        let promoted = vm.promote_if_needed_for_write_ptr(target, value);

        assert!(!is_epoch_object(&vm, promoted));
        assert!(unsafe { (&*promoted.as_js_object_ptr()).is_session_epoch() });
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
