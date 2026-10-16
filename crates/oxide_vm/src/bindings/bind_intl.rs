use crate::bindings::{
    apply_binding_table, bind_global_value, bind_well_known_data_property, configure_native_constructor, BindEnv,
};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 一组 Intl 构造器/原型对：（构造器槽，原型槽，构造器 native 函数，
/// 构造器 length 槽，命名空间槽名，原型标签）。
type IntlCtorPair<'a> = (
    &'a oxide_types::mem::P<JsObject>,
    &'a oxide_types::mem::P<JsObject>,
    *const (),
    u32,
    &'a str,
    &'a str,
);

/// 把 Intl 命名空间对象与十组构造器/原型对绑定到 global。
///
/// `realm_id` 是目标所属 realm 的编号：`@@toStringTag` 符号键按 (realm 编号,
/// 局部下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
///
/// 构造器/原型对与 `prototype`/`constructor` 互指、命名空间与构造器的
/// `[[Prototype]]` 链由内核接线（`wire_builtin_world_links`）预置，本函数
/// 只安装 native 函数面、`length` 与 `@@toStringTag`。
pub fn bind_intl(env: BindEnv<'_>, global: &mut JsObject) {
    let world = env.world();
    let intl_ptr = world.intl_object.as_ptr() as *mut JsObject;
    let intl = unsafe { &mut *intl_ptr };

    // Intl.getCanonicalLocales 与 Intl.supportedValuesOf：命名空间对象上的
    // 静态方法（各 length 1，非构造器）。
    apply_binding_table(
        env,
        intl,
        &[
            (
                "getCanonicalLocales",
                oxide_builtins::intl::get_canonical_locales::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "supportedValuesOf",
                oxide_builtins::intl::supported_values_of::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
    bind_well_known_data_property(
        env.core,
        intl,
        9,
        JsValue::perm_string(env.string_forge().string_ptr(env.string_forge().intern("Intl").0)),
        PropAttributes::new(false, false, true),
        env.realm_id,
    );

    // 十组构造器/原型对：构造器（length 0）+ supportedLocalesOf 静态方法 +
    // 命名空间槽（Intl.NumberFormat 等）+ 原型 @@toStringTag。
    let pairs: [IntlCtorPair; 10] = [
        (
            &world.number_format_constructor,
            &world.number_format_proto,
            oxide_builtins::intl::number_format_constructor::<crate::vm::Vm> as *const (),
            0,
            "NumberFormat",
            "Intl.NumberFormat",
        ),
        (
            &world.date_time_format_constructor,
            &world.date_time_format_proto,
            oxide_builtins::intl::date_time_format_constructor::<crate::vm::Vm> as *const (),
            0,
            "DateTimeFormat",
            "Intl.DateTimeFormat",
        ),
        (
            &world.collator_constructor,
            &world.collator_proto,
            oxide_builtins::intl::collator_constructor::<crate::vm::Vm> as *const (),
            0,
            "Collator",
            "Intl.Collator",
        ),
        (
            &world.plural_rules_constructor,
            &world.plural_rules_proto,
            oxide_builtins::intl::plural_rules_constructor::<crate::vm::Vm> as *const (),
            0,
            "PluralRules",
            "Intl.PluralRules",
        ),
        (
            &world.list_format_constructor,
            &world.list_format_proto,
            oxide_builtins::intl::list_format_constructor::<crate::vm::Vm> as *const (),
            0,
            "ListFormat",
            "Intl.ListFormat",
        ),
        (
            &world.relative_time_format_constructor,
            &world.relative_time_format_proto,
            oxide_builtins::intl::relative_time_format_constructor::<crate::vm::Vm> as *const (),
            0,
            "RelativeTimeFormat",
            "Intl.RelativeTimeFormat",
        ),
        (
            &world.display_names_constructor,
            &world.display_names_proto,
            oxide_builtins::intl::display_names_constructor::<crate::vm::Vm> as *const (),
            2,
            "DisplayNames",
            "Intl.DisplayNames",
        ),
        (
            &world.segmenter_constructor,
            &world.segmenter_proto,
            oxide_builtins::intl::segmenter_constructor::<crate::vm::Vm> as *const (),
            0,
            "Segmenter",
            "Intl.Segmenter",
        ),
        (
            &world.locale_constructor,
            &world.locale_proto,
            oxide_builtins::intl::locale_constructor::<crate::vm::Vm> as *const (),
            1,
            "Locale",
            "Intl.Locale",
        ),
        (
            &world.duration_format_constructor,
            &world.duration_format_proto,
            oxide_builtins::intl::duration_format_constructor::<crate::vm::Vm> as *const (),
            0,
            "DurationFormat",
            "Intl.DurationFormat",
        ),
    ];
    for (ctor_p, proto_p, ctor_fn, length, name, tag) in pairs {
        let ctor = unsafe { &mut *(ctor_p.as_ptr() as *mut JsObject) };
        let proto = unsafe { &mut *(proto_p.as_ptr() as *mut JsObject) };
        bind_intl_ctor_pair(env, ctor, proto, ctor_fn, length, tag);
        // 命名空间对象上的构造器槽（如 Intl.NumberFormat）：既有槽原位更新。
        bind_global_value(env.core, intl, name, JsValue::from_js_object(ctor_p.as_ptr() as *mut JsObject));
    }

    // 命名空间对象挂到 global（既有槽原位更新，幂等）。
    bind_global_value(env.core, global, "Intl", JsValue::from_js_object(intl_ptr));
}

/// 安装一组 Intl 构造器/原型对：构造器 native 函数与 `length`、
/// `supportedLocalesOf` 静态方法、原型 `@@toStringTag`。
fn bind_intl_ctor_pair(
    env: BindEnv<'_>, ctor: &mut JsObject, proto: &mut JsObject, ctor_fn: *const (), length: u32, tag: &str,
) {
    // 构造器：length 槽的 native 函数；`prototype`/`name` 槽由内核接线
    // 预置（wire_ctor_proto / make_pair），此处不重复建槽。
    configure_native_constructor(ctor, ctor_fn, 0);
    let length_si = env.string_forge().intern("length").0;
    let length_shape = env.shape_forge().make_shape(ctor.shape_id(), length_si);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(length as i32));
    let length_pos = ctor.hash_props_vec().map_or(0, |props| props.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    // supportedLocalesOf：普通静态方法（非构造器、无 prototype），length 1。
    apply_binding_table(
        env,
        ctor,
        &[(
            "supportedLocalesOf",
            oxide_builtins::intl::supported_locales_of::<crate::vm::Vm> as *const (),
            1,
        )],
    );

    // 原型 @@toStringTag：{ writable:false, enumerable:false, configurable:true }。
    bind_well_known_data_property(
        env.core,
        proto,
        9,
        JsValue::perm_string(env.string_forge().string_ptr(env.string_forge().intern(tag).0)),
        PropAttributes::new(false, false, true),
        env.realm_id,
    );
}
