//! native 载荷家族的单点分类与每家族边函数引用（家族表）。
//!
//! 分类是复合谓词：header 位（map/set/module_ns）优先于 `type_tag`，
//! 两维互斥、一个对象至多归入一个家族。tag 维显式枚举全部 0..=38，
//! 未登记 tag panic。
//!
//! 关键约定：
//! - 每家族对 object/string/cell 三类边至多各持一个函数引用；无 native
//!   盒的类型显式归空家族。
//! - 注册面集中在 `ops_for` 的家族表，mark/size/drop/clone/rewrite 各链
//!   由同一张表驱动，新增家族只在此登记。

use oxide_builtins::{data_view, disposable_stack, event, map, message_channel, module, set, typed_array, weak_map};
use oxide_types::object::{Cell, JsObject, JsString};
use oxide_types::value::JsValue;

/// native 载荷家族。
///
/// 21 个 native 家族 + 空家族；空家族是无 native 盒类型的显式归类结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeBoxFamily {
    /// 无 native 载荷盒。
    None,
    /// Map 实例（header bit 25）。
    Map,
    /// Set 实例（header bit 24）。
    Set,
    /// 模块命名空间 exotic 对象（`_pad` bit 2）。
    ModuleNamespace,
    /// WeakMap（tag 31，条目表）。
    WeakMap,
    /// DisposableStack / AsyncDisposableStack（tag 24/25）。
    DisposableStack,
    /// TypedArray（tag 8）。
    TypedArray,
    /// DataView（tag 7）。
    DataView,
    /// ArrayBuffer（tag 6，字节载荷、无引用边）。
    ArrayBuffer,
    /// SharedArrayBuffer（tag 30，字节载荷、无引用边）。
    SharedArrayBuffer,
    /// 生成器迭代器（tag 10）。
    Generator,
    /// Promise（tag 11）。
    Promise,
    /// 异步函数执行上下文（tag 12）。
    Async,
    /// 异步生成器迭代器（tag 13）。
    AsyncGenerator,
    /// RegExp 与 matchAll 载体（tag 2/28，`native_fn` 槽持编译正则）。
    RegExp,
    /// MessagePort 端口对象（tag 33，mpsc 双端与对端端口边）。
    MessagePort,
    /// BroadcastChannel 通道对象（tag 34，mpsc 发送/接收对与通道名，无对象边）。
    BroadcastChannel,
    /// Event 事件基类对象（tag 35，type 串与九项状态位载荷盒）。
    Event,
    /// MessageEvent 事件派生类对象（tag 36，data 与 ports 边）。
    MessageEvent,
    /// ErrorEvent 事件派生类对象（tag 37，message 与 error 边）。
    ErrorEvent,
    /// CustomEvent 事件派生类对象（tag 38，detail 边）。
    CustomEvent,
    /// mapped arguments 对象（tag 20，同步状态盒存 `native_data`，无引用边）。
    Arguments,
}

/// 一个家族的边操作集（每家族函数引用，单一注册面）。
pub(crate) struct NativeBoxOps {
    /// 对象强边（mark 栈 / sweep 改写）。
    pub(crate) object_edges: Option<fn(&JsObject) -> Vec<JsValue>>,
    /// native 字符串边（mark 存活集）。
    pub(crate) string_edges: Option<fn(&JsObject) -> Vec<*mut JsString>>,
    /// native cell 边（mark 存活集）。
    pub(crate) cell_edges: Option<fn(&JsObject) -> Vec<*mut Cell>>,
}

/// 分类对象的 native 载荷家族。
///
/// header 位优先：map/set/module_ns 经 header 位判定，与 tag 维正交；
/// 其余按 `type_tag` 分类。
pub(crate) fn classify(obj: &JsObject) -> NativeBoxFamily {
    if obj.is_map() {
        return NativeBoxFamily::Map;
    }
    if obj.is_set() {
        return NativeBoxFamily::Set;
    }
    if obj.is_module_namespace() {
        return NativeBoxFamily::ModuleNamespace;
    }
    classify_by_tag(obj.type_tag)
}

/// 按类型 tag 分类 native 载荷家族。
///
/// # 边界与前提
/// - 0..=38 全 tag 显式枚举；未登记 tag（39 及以上）panic。
pub(crate) fn classify_by_tag(tag: u8) -> NativeBoxFamily {
    match tag {
        JsObject::OBJ_TYPE_ARRAY_BUFFER => NativeBoxFamily::ArrayBuffer,
        JsObject::OBJ_TYPE_DATA_VIEW => NativeBoxFamily::DataView,
        JsObject::OBJ_TYPE_TYPED_ARRAY => NativeBoxFamily::TypedArray,
        JsObject::OBJ_TYPE_GENERATOR => NativeBoxFamily::Generator,
        JsObject::OBJ_TYPE_PROMISE => NativeBoxFamily::Promise,
        JsObject::OBJ_TYPE_ASYNC => NativeBoxFamily::Async,
        JsObject::OBJ_TYPE_ASYNC_GENERATOR => NativeBoxFamily::AsyncGenerator,
        JsObject::OBJ_TYPE_DISPOSABLE_STACK | JsObject::OBJ_TYPE_ASYNC_DISPOSABLE_STACK => {
            NativeBoxFamily::DisposableStack
        }
        JsObject::OBJ_TYPE_WEAK_MAP => NativeBoxFamily::WeakMap,
        JsObject::OBJ_TYPE_SHARED_ARRAY_BUFFER => NativeBoxFamily::SharedArrayBuffer,
        JsObject::OBJ_TYPE_REGEXP | JsObject::OBJ_TYPE_REGEX_STUB => NativeBoxFamily::RegExp,
        // 以下 tag 无 native 载荷盒，显式归空家族。
        JsObject::OBJ_TYPE_PLAIN => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_DATE => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_BOOLEAN_OBJ => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_NUMBER_OBJ => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_STRING_OBJ => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_CONSTRUCTOR => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_INSTANT => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_PLAIN_DATE => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_PLAIN_TIME => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_DURATION => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_ZONED_DATE_TIME => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_PLAIN_DATE_TIME => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_ARGUMENTS => NativeBoxFamily::Arguments,
        JsObject::OBJ_TYPE_SYMBOL_OBJ => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_ERROR => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_BOUND => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_PLAIN_MONTH_DAY => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_HTML_DDA => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_RAW_JSON => NativeBoxFamily::None,
        JsObject::OBJ_TYPE_MESSAGE_PORT => NativeBoxFamily::MessagePort,
        JsObject::OBJ_TYPE_BROADCAST_CHANNEL => NativeBoxFamily::BroadcastChannel,
        JsObject::OBJ_TYPE_EVENT => NativeBoxFamily::Event,
        JsObject::OBJ_TYPE_MESSAGE_EVENT => NativeBoxFamily::MessageEvent,
        JsObject::OBJ_TYPE_ERROR_EVENT => NativeBoxFamily::ErrorEvent,
        JsObject::OBJ_TYPE_CUSTOM_EVENT => NativeBoxFamily::CustomEvent,
        _ => panic!("unregistered type tag: {tag}"),
    }
}

/// RegExp 家族的对象边：source/flags 字段（matchAll 载体无可信属性面，
/// 两字段恒 undefined，边自然为空）。
fn regexp_object_edges(obj: &JsObject) -> Vec<JsValue> {
    vec![obj.get_regexp_source(), obj.get_regexp_flags()]
}

/// 查家族的边操作集（家族表，单一注册面）。
pub(crate) fn ops_for(family: NativeBoxFamily) -> NativeBoxOps {
    match family {
        NativeBoxFamily::None => NativeBoxOps {
            object_edges: None,
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::Map => NativeBoxOps {
            object_edges: Some(map::map_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::Set => NativeBoxOps {
            object_edges: Some(set::set_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::ModuleNamespace => NativeBoxOps {
            object_edges: Some(module::module_ns_native_edges),
            string_edges: None,
            cell_edges: Some(module::module_ns_cell_edges),
        },
        // WeakMap 仅值边进 mark（强边）；键为弱边，不入栈不置位。
        NativeBoxFamily::WeakMap => NativeBoxOps {
            object_edges: Some(weak_map::weak_map_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::DisposableStack => NativeBoxOps {
            object_edges: Some(disposable_stack::dispose_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::TypedArray => NativeBoxOps {
            object_edges: Some(typed_array::typed_array_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::DataView => NativeBoxOps {
            object_edges: Some(data_view::data_view_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        // 字节载荷盒：无引用边，仅 size/drop 链消费。
        NativeBoxFamily::ArrayBuffer | NativeBoxFamily::SharedArrayBuffer => NativeBoxOps {
            object_edges: None,
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::Generator => NativeBoxOps {
            object_edges: Some(crate::generator::generator_native_edges),
            string_edges: Some(crate::generator::generator_native_string_edges),
            cell_edges: Some(crate::generator::generator_native_cell_edges),
        },
        NativeBoxFamily::Promise => NativeBoxOps {
            object_edges: Some(crate::promise::promise_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::Async => NativeBoxOps {
            object_edges: Some(crate::async_func::async_native_edges),
            string_edges: Some(crate::async_func::async_native_string_edges),
            cell_edges: Some(crate::async_func::async_native_cell_edges),
        },
        NativeBoxFamily::AsyncGenerator => NativeBoxOps {
            object_edges: Some(crate::async_generator::async_generator_native_edges),
            string_edges: Some(crate::async_generator::async_generator_native_string_edges),
            cell_edges: Some(crate::async_generator::async_generator_native_cell_edges),
        },
        NativeBoxFamily::RegExp => NativeBoxOps {
            object_edges: Some(regexp_object_edges),
            string_edges: None,
            cell_edges: None,
        },
        NativeBoxFamily::MessagePort => NativeBoxOps {
            object_edges: Some(message_channel::message_port_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        // BroadcastChannel 载荷盒：无对象边（mpsc 非 GC 边、通道名为 Rust String），
        // 三边函数全空，仅 size/drop 链消费。
        NativeBoxFamily::BroadcastChannel => NativeBoxOps {
            object_edges: None,
            string_edges: None,
            cell_edges: None,
        },
        // Event 基类载荷盒：type 串边与 target / current_target 对象边。
        NativeBoxFamily::Event => NativeBoxOps {
            object_edges: Some(event::event_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        // MessageEvent 派生类载荷盒：基类 type / target / current_target 边加
        // data / source / ports 边。
        NativeBoxFamily::MessageEvent => NativeBoxOps {
            object_edges: Some(event::message_event_native_edges),
            string_edges: None,
            cell_edges: None,
        },
        // ErrorEvent / CustomEvent 载荷盒由后续子任务填充；本两臂当前无引用
        // 边，仅 size/drop 链消费。
        NativeBoxFamily::ErrorEvent | NativeBoxFamily::CustomEvent => NativeBoxOps {
            object_edges: None,
            string_edges: None,
            cell_edges: None,
        },
        // mapped arguments 同步状态盒：无引用边（位图与帧身份均为原始值），
        // 三边函数全空，仅 size/drop 链消费。
        NativeBoxFamily::Arguments => NativeBoxOps {
            object_edges: None,
            string_edges: None,
            cell_edges: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj_with_tag(tag: u8) -> JsObject {
        let mut obj = JsObject::new_empty(1, JsValue::undefined());
        obj.type_tag = tag;
        obj
    }

    /// tag 维期望家族（测试侧口径，与分类器镜像对照）。
    fn expected_family_for_tag(tag: u8) -> NativeBoxFamily {
        match tag {
            JsObject::OBJ_TYPE_ARRAY_BUFFER => NativeBoxFamily::ArrayBuffer,
            JsObject::OBJ_TYPE_DATA_VIEW => NativeBoxFamily::DataView,
            JsObject::OBJ_TYPE_TYPED_ARRAY => NativeBoxFamily::TypedArray,
            JsObject::OBJ_TYPE_GENERATOR => NativeBoxFamily::Generator,
            JsObject::OBJ_TYPE_PROMISE => NativeBoxFamily::Promise,
            JsObject::OBJ_TYPE_ASYNC => NativeBoxFamily::Async,
            JsObject::OBJ_TYPE_ASYNC_GENERATOR => NativeBoxFamily::AsyncGenerator,
            JsObject::OBJ_TYPE_DISPOSABLE_STACK | JsObject::OBJ_TYPE_ASYNC_DISPOSABLE_STACK => {
                NativeBoxFamily::DisposableStack
            }
            JsObject::OBJ_TYPE_WEAK_MAP => NativeBoxFamily::WeakMap,
            JsObject::OBJ_TYPE_SHARED_ARRAY_BUFFER => NativeBoxFamily::SharedArrayBuffer,
            JsObject::OBJ_TYPE_REGEXP | JsObject::OBJ_TYPE_REGEX_STUB => NativeBoxFamily::RegExp,
            JsObject::OBJ_TYPE_MESSAGE_PORT => NativeBoxFamily::MessagePort,
            JsObject::OBJ_TYPE_BROADCAST_CHANNEL => NativeBoxFamily::BroadcastChannel,
            JsObject::OBJ_TYPE_EVENT => NativeBoxFamily::Event,
            JsObject::OBJ_TYPE_MESSAGE_EVENT => NativeBoxFamily::MessageEvent,
            JsObject::OBJ_TYPE_ERROR_EVENT => NativeBoxFamily::ErrorEvent,
            JsObject::OBJ_TYPE_CUSTOM_EVENT => NativeBoxFamily::CustomEvent,
            JsObject::OBJ_TYPE_ARGUMENTS => NativeBoxFamily::Arguments,
            _ => NativeBoxFamily::None,
        }
    }

    #[test]
    fn tag_dimension_every_tag_hits_exactly_one_family() {
        for tag in 0..=38u8 {
            let obj = obj_with_tag(tag);
            let family = classify(&obj);
            assert_eq!(family, expected_family_for_tag(tag), "tag {tag} 家族不一致");
        }
    }

    #[test]
    fn header_bit_families_are_independent_dimension() {
        // map/set/module_ns 经 header 位判定，tag 为 PLAIN，与 tag 维正交。
        let mut map_obj = obj_with_tag(JsObject::OBJ_TYPE_PLAIN);
        map_obj.set_map(true);
        assert_eq!(classify(&map_obj), NativeBoxFamily::Map);

        let mut set_obj = obj_with_tag(JsObject::OBJ_TYPE_PLAIN);
        set_obj.set_set(true);
        assert_eq!(classify(&set_obj), NativeBoxFamily::Set);

        let mut ns_obj = obj_with_tag(JsObject::OBJ_TYPE_PLAIN);
        ns_obj.set_module_namespace(true);
        assert_eq!(classify(&ns_obj), NativeBoxFamily::ModuleNamespace);
    }

    #[test]
    fn header_bits_take_priority_over_tag() {
        // header 位优先于 tag：同置两维时按 header 位归类。
        let mut obj = obj_with_tag(JsObject::OBJ_TYPE_ARRAY_BUFFER);
        obj.set_map(true);
        assert_eq!(classify(&obj), NativeBoxFamily::Map);
    }

    #[test]
    #[should_panic(expected = "unregistered type tag")]
    fn unregistered_tag_panics() {
        let obj = obj_with_tag(39);
        classify(&obj);
    }
}
