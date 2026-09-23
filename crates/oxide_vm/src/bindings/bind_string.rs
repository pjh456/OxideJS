use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::builtin::StringMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 String 构造器与原型方法绑定到 global。
pub fn bind_string(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let string_methods = StringMethods {
        from_char_code: oxide_builtins::string::string_from_char_code::<crate::vm::Vm> as *const (),
        index_of: oxide_builtins::string::string_index_of::<crate::vm::Vm> as *const (),
        includes: oxide_builtins::string::string_includes::<crate::vm::Vm> as *const (),
        char_at: oxide_builtins::string::string_char_at::<crate::vm::Vm> as *const (),
        char_code_at: oxide_builtins::string::string_char_code_at::<crate::vm::Vm> as *const (),
        concat: oxide_builtins::string::string_concat::<crate::vm::Vm> as *const (),
        slice: oxide_builtins::string::string_slice::<crate::vm::Vm> as *const (),
        substring: oxide_builtins::string::string_substring::<crate::vm::Vm> as *const (),
        to_upper_case: oxide_builtins::string::string_to_upper_case::<crate::vm::Vm> as *const (),
        to_lower_case: oxide_builtins::string::string_to_lower_case::<crate::vm::Vm> as *const (),
        to_locale_upper_case: oxide_builtins::string::string_to_locale_upper_case::<crate::vm::Vm> as *const (),
        to_locale_lower_case: oxide_builtins::string::string_to_locale_lower_case::<crate::vm::Vm> as *const (),
        locale_compare: oxide_builtins::string::string_locale_compare::<crate::vm::Vm> as *const (),
        trim: oxide_builtins::string::string_trim::<crate::vm::Vm> as *const (),
        repeat: oxide_builtins::string::string_repeat::<crate::vm::Vm> as *const (),
        pad_start: oxide_builtins::string::string_pad_start::<crate::vm::Vm> as *const (),
        pad_end: oxide_builtins::string::string_pad_end::<crate::vm::Vm> as *const (),
        starts_with: oxide_builtins::string::string_starts_with::<crate::vm::Vm> as *const (),
        ends_with: oxide_builtins::string::string_ends_with::<crate::vm::Vm> as *const (),
        split: oxide_builtins::string::string_split::<crate::vm::Vm> as *const (),
        replace: oxide_builtins::string::string_replace::<crate::vm::Vm> as *const (),
        match_fn: oxide_builtins::string::string_match_fn::<crate::vm::Vm> as *const (),
        search: oxide_builtins::string::string_search::<crate::vm::Vm> as *const (),
        trim_start: oxide_builtins::string::string_trim_start::<crate::vm::Vm> as *const (),
        trim_end: oxide_builtins::string::string_trim_end::<crate::vm::Vm> as *const (),
        code_point_at: oxide_builtins::string::string_code_point_at::<crate::vm::Vm> as *const (),
        normalize: oxide_builtins::string::string_normalize::<crate::vm::Vm> as *const (),
        match_all: oxide_builtins::string::string_match_all::<crate::vm::Vm> as *const (),
        replace_all: oxide_builtins::string::string_replace_all::<crate::vm::Vm> as *const (),
        value_of: oxide_builtins::string::string_value_of::<crate::vm::Vm> as *const (),
        substr: oxide_builtins::string::string_substr::<crate::vm::Vm> as *const (),
        at: oxide_builtins::string::string_at::<crate::vm::Vm> as *const (),
        last_index_of: oxide_builtins::string::string_last_index_of::<crate::vm::Vm> as *const (),
        from_code_point: oxide_builtins::string::string_from_code_point::<crate::vm::Vm> as *const (),
        is_well_formed: oxide_builtins::string::string_is_well_formed::<crate::vm::Vm> as *const (),
        to_well_formed: oxide_builtins::string::string_to_well_formed::<crate::vm::Vm> as *const (),
        from_raw: oxide_builtins::string::string_raw::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_string_methods(
        &string_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let ctor_ptr = session.builtin_world().string_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    configure_native_constructor(ctor, oxide_builtins::string::string_constructor::<crate::vm::Vm> as *const (), 1);
    // String.length = 1，描述符 { writable:false, enumerable:false, configurable:true }。
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(ctor.shape_id(), length_si);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(1));
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    let proto_ptr = session.builtin_world().string_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[("toString", oxide_builtins::string::string_to_string::<crate::vm::Vm> as *const (), 0)],
    );
    // String.prototype[@@iterator]：逐 code point 迭代字符。
    super::bind_well_known_method(
        session.builtin_world(),
        core,
        proto,
        0,
        "iterator",
        oxide_builtins::string::string_symbol_iterator::<crate::vm::Vm> as *const (),
        0,
    );
    // 捕获默认迭代器函数对象指针写入 world：String 臂覆盖判定以此做指针比较。
    // 此时槽为初始数据属性（用户覆盖尚未可能），读回即默认函数本体。
    let iter_key = oxide_types::private_key::make_well_known_symbol_key(0);
    if let Some(pos) = core.shape_forge().lookup_position(proto.shape_id(), iter_key) {
        let v = proto.get_prop_at(pos);
        if v.is_object() {
            session
                .builtin_world()
                .string_default_iterator
                .set(v.as_js_object_ptr() as *const JsObject);
        }
    }

    // 全局 String 槽位既有槽原位更新（旧家族构造器指针不得滞留在属性 vec），
    // 无槽时开新槽；描述符非枚举（规范 { writable:true, enumerable:false, configurable:true }）。
    let si_str = core.perm_interner().intern("String").0;
    let str_val = JsValue::from_js_object(session.builtin_world().string_constructor.as_ptr() as *mut JsObject);
    if let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), si_str) {
        global.set_prop_at(pos, str_val);
    } else {
        let str_shape = core.shape_forge().make_shape(global.shape_id(), si_str);
        global.set_shape_id(str_shape);
        global.ensure_hash_props().push(str_val);
        let pos = global.prop_vec_len().saturating_sub(1) as u32;
        global.set_data_meta(pos, PropAttributes::new(true, false, true));
        global.bump_generation();
    }
}
