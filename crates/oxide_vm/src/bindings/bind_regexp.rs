use std::sync::Arc;

use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_accessor_getset, bind_accessor_getter_key,
    bind_well_known_method, configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

/// 把 RegExp 构造器与原型方法绑定到 global。
///
/// `realm_id` 是目标所属 realm 的编号：well-known 符号键按 (realm 编号, 局部
/// 下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
pub fn bind_regexp(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, realm_id: u32) {
    let ctor_ptr = session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm> as *const (), 2);

    // 静态方法 escape 装到构造器对象（wrapper 的 length/name 与槽属性由 bind_method 统一保证）。
    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[("escape", oxide_builtins::regexp::regexp_escape::<crate::vm::Vm> as *const (), 1)],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("exec", oxide_builtins::regexp::regexp_exec::<crate::vm::Vm> as *const (), 1),
            ("test", oxide_builtins::regexp::regexp_test::<crate::vm::Vm> as *const (), 1),
            ("toString", oxide_builtins::regexp::regexp_to_string::<crate::vm::Vm> as *const (), 0),
            ("compile", oxide_builtins::regexp::regexp_compile::<crate::vm::Vm> as *const (), 2),
        ],
    );

    // 遗留静态访问器（Annex B）：15 个访问器落 %RegExp% 构造器，描述符
    // {enumerable:false, configurable:true}。仅 input/$_ 为 get+set 对（共享
    // [[RegExpInput]] 槽）；其余 18 个为 getter（set 恒 undefined）：lastMatch/$&、
    // lastParen/$+、leftContext/$`、rightContext/$' 各共享一槽，index 与 $1-$9 各
    // 独占一槽。
    let sf = core.perm_interner().as_ref();
    let mut bind_getset = |name: &str, getter: *const (), setter: *const ()| {
        let key = sf.intern(name).0;
        bind_accessor_getset(
            core,
            session,
            ctor,
            key,
            &format!("get {name}"),
            &format!("set {name}"),
            getter,
            setter,
        );
    };
    // input/$_：get+set 对（唯一带 setter 的遗留访问器）。
    bind_getset(
        "input",
        oxide_builtins::regexp::regexp_legacy_get_input::<crate::vm::Vm> as *const (),
        oxide_builtins::regexp::regexp_legacy_set_input::<crate::vm::Vm> as *const (),
    );
    bind_getset(
        "$_",
        oxide_builtins::regexp::regexp_legacy_get_input::<crate::vm::Vm> as *const (),
        oxide_builtins::regexp::regexp_legacy_set_input::<crate::vm::Vm> as *const (),
    );
    // 其余 18 个 getter（set 恒 undefined）。
    for (name, getter) in [
        ("lastMatch", oxide_builtins::regexp::regexp_legacy_get_last_match::<crate::vm::Vm> as *const ()),
        ("$&", oxide_builtins::regexp::regexp_legacy_get_last_match::<crate::vm::Vm> as *const ()),
        ("lastParen", oxide_builtins::regexp::regexp_legacy_get_last_paren::<crate::vm::Vm> as *const ()),
        ("$+", oxide_builtins::regexp::regexp_legacy_get_last_paren::<crate::vm::Vm> as *const ()),
        (
            "leftContext",
            oxide_builtins::regexp::regexp_legacy_get_left_context::<crate::vm::Vm> as *const (),
        ),
        ("$`", oxide_builtins::regexp::regexp_legacy_get_left_context::<crate::vm::Vm> as *const ()),
        (
            "rightContext",
            oxide_builtins::regexp::regexp_legacy_get_right_context::<crate::vm::Vm> as *const (),
        ),
        ("$'", oxide_builtins::regexp::regexp_legacy_get_right_context::<crate::vm::Vm> as *const ()),
        ("index", oxide_builtins::regexp::regexp_legacy_get_index::<crate::vm::Vm> as *const ()),
        ("$1", oxide_builtins::regexp::regexp_legacy_get_dollar_1::<crate::vm::Vm> as *const ()),
        ("$2", oxide_builtins::regexp::regexp_legacy_get_dollar_2::<crate::vm::Vm> as *const ()),
        ("$3", oxide_builtins::regexp::regexp_legacy_get_dollar_3::<crate::vm::Vm> as *const ()),
        ("$4", oxide_builtins::regexp::regexp_legacy_get_dollar_4::<crate::vm::Vm> as *const ()),
        ("$5", oxide_builtins::regexp::regexp_legacy_get_dollar_5::<crate::vm::Vm> as *const ()),
        ("$6", oxide_builtins::regexp::regexp_legacy_get_dollar_6::<crate::vm::Vm> as *const ()),
        ("$7", oxide_builtins::regexp::regexp_legacy_get_dollar_7::<crate::vm::Vm> as *const ()),
        ("$8", oxide_builtins::regexp::regexp_legacy_get_dollar_8::<crate::vm::Vm> as *const ()),
        ("$9", oxide_builtins::regexp::regexp_legacy_get_dollar_9::<crate::vm::Vm> as *const ()),
    ] {
        bind_accessor_getter(core, session, ctor, name, getter);
    }

    // source/flags 只读访问器（set 恒 undefined，getter 读实例字段）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "source",
        oxide_builtins::regexp::regexp_get_source::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "flags",
        oxide_builtins::regexp::regexp_get_flags::<crate::vm::Vm> as *const (),
    );

    // 8 个单 flag 只读访问器（set 恒 undefined，getter 读实例 flags 串判码元）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "global",
        oxide_builtins::regexp::regexp_get_global::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "ignoreCase",
        oxide_builtins::regexp::regexp_get_ignore_case::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "multiline",
        oxide_builtins::regexp::regexp_get_multiline::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "dotAll",
        oxide_builtins::regexp::regexp_get_dot_all::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "sticky",
        oxide_builtins::regexp::regexp_get_sticky::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "unicode",
        oxide_builtins::regexp::regexp_get_unicode::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "hasIndices",
        oxide_builtins::regexp::regexp_get_has_indices::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "unicodeSets",
        oxide_builtins::regexp::regexp_get_unicode_sets::<crate::vm::Vm> as *const (),
    );

    // well-known symbol 方法按 Symbol 键安装，name 属性取 `[Symbol.match]` 式标签，供 `re[Symbol.match]` 等读取。
    let world = session.builtin_world();
    bind_well_known_method(
        world,
        core,
        proto,
        1,
        "[Symbol.match]",
        oxide_builtins::regexp::regexp_symbol_match::<crate::vm::Vm> as *const (),
        1,
        realm_id,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        2,
        "[Symbol.replace]",
        oxide_builtins::regexp::regexp_symbol_replace::<crate::vm::Vm> as *const (),
        2,
        realm_id,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        3,
        "[Symbol.search]",
        oxide_builtins::regexp::regexp_symbol_search::<crate::vm::Vm> as *const (),
        1,
        realm_id,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        4,
        "[Symbol.split]",
        oxide_builtins::regexp::regexp_symbol_split::<crate::vm::Vm> as *const (),
        2,
        realm_id,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        7,
        "[Symbol.matchAll]",
        oxide_builtins::regexp::regexp_symbol_match_all::<crate::vm::Vm> as *const (),
        1,
        realm_id,
    );

    // RegExp[Symbol.species] 访问器：getter 返回 receiver，派生类沿静态原型链解析
    // @@species 得自身构造器（规范不给 class 默认 static @@species，类上无 own）。
    bind_accessor_getter_key(
        core,
        session,
        ctor,
        oxide_types::private_key::encode_symbol_key(realm_id, oxide_types::private_key::WELL_KNOWN_SYMBOL_SPECIES),
        "get [Symbol.species]",
        oxide_builtins::array::array_species_get::<crate::vm::Vm> as *const (),
    );

    bind_constructor!(core, global, "RegExp", ctor_ptr, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm>, 2, hash: true);
}
