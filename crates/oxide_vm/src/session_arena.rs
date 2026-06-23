use std::collections::HashMap;

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtins::{data_view, map, set, typed_array};
use crate::vm::Vm;

impl Vm {
    pub(crate) fn is_session_escape_root_ptr(&self, target_ptr: *mut JsObject) -> bool {
        if target_ptr.is_null() {
            return false;
        }
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        std::ptr::eq(target_ptr, global_ptr) || unsafe { (&*target_ptr).is_session_epoch() }
    }

    pub(crate) fn promote_object(&mut self, src: *mut JsObject) -> *mut JsObject {
        let mut forwarding = HashMap::new();
        self.promote_object_inner(src, &mut forwarding)
    }

    pub(crate) fn promote_object_inner(
        &mut self, src: *mut JsObject, forwarding: &mut HashMap<*mut JsObject, *mut JsObject>,
    ) -> *mut JsObject {
        if src.is_null() {
            return src;
        }
        let src_ref = unsafe { &*src };
        if src_ref.is_session_epoch() || !self.epoch.is_epoch_ptr(src.cast::<u8>()) {
            return src;
        }
        if let Some(dst) = forwarding.get(&src).copied() {
            return dst;
        }

        let clone = src_ref.clone_for_session_epoch();
        let dst = self.session_epoch.alloc(clone) as *mut JsObject;
        forwarding.insert(src, dst);
        self.session_object_ptrs.push(dst);
        self.session_bytes_allocated += std::mem::size_of::<JsObject>();

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
        } else if src_ref.is_typed_array_obj() {
            typed_array::clone_typed_array_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        } else if src_ref.is_data_view_obj() {
            data_view::clone_data_view_native_with_rewrite(src_ref, dst_ref, |value| {
                self.promote_value_if_epoch_object(value, forwarding)
            });
        }
        dst
    }

    pub(crate) fn promote_value_if_epoch_object(
        &mut self, value: JsValue, forwarding: &mut HashMap<*mut JsObject, *mut JsObject>,
    ) -> JsValue {
        if !value.is_object() {
            return value;
        }
        let ptr = value.as_js_object_ptr();
        if ptr.is_null() || !self.epoch.is_epoch_ptr(ptr.cast::<u8>()) {
            return value;
        }
        JsValue::from_js_object(self.promote_object_inner(ptr, forwarding))
    }

    pub(crate) fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue {
        if !value.is_object() || !self.is_session_escape_root_ptr(target_ptr) {
            return value;
        }
        let value_ptr = value.as_js_object_ptr();
        if value_ptr.is_null() || !self.epoch.is_epoch_ptr(value_ptr.cast::<u8>()) {
            return value;
        }
        JsValue::from_js_object(self.promote_object(value_ptr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_types::object::{PropAttributes, PropMetaEntry};

    fn plain_object(vm: &mut Vm) -> *mut JsObject {
        let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        vm.epoch
            .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)))
    }

    fn is_epoch_object(vm: &Vm, value: JsValue) -> bool {
        value.is_object() && vm.epoch.is_epoch_ptr(value.as_js_object_ptr().cast::<u8>())
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
}
