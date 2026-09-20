use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_string, to_string_full, NativeResult, VmHost};
use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

fn set_own_message<H: VmHost>(host: &mut H, this: *mut JsObject, args: &[u8]) {
    if args.len() <= 1 {
        return;
    }
    let msg_val = host.reg(args[1]);
    if msg_val.is_undefined() {
        return;
    }
    let msg_str = to_string(msg_val);
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());
    let si = sf.intern("message").0;
    let new_shape = sh.make_shape(EMPTY_SHAPE_ID, si);
    let perm_val = host.new_string(&msg_str);
    unsafe {
        (*this).set_shape_id(new_shape);
        let pos = (*this).push_prop(perm_val);
        // message 按规范为非枚举数据属性（CreateNonEnumerableDataPropertyOrThrow）。
        (*this).set_data_meta(pos, PropAttributes::new(true, false, true));
    }
}

/// 按错误类型名创建对应 Error 对象（message 非空时写为自身属性）。
/// 这是引擎内部构造错误的统一入口，供 VM/native 层抛错使用。
pub fn create_kind_error<H: VmHost>(host: &mut H, kind: &str, msg: &str) -> JsValue {
    let proto_ptr = match kind {
        "TypeError" => P::as_ptr(&host.session().builtin_world().type_error_proto) as *mut JsObject,
        "RangeError" => P::as_ptr(&host.session().builtin_world().range_error_proto) as *mut JsObject,
        "ReferenceError" => P::as_ptr(&host.session().builtin_world().reference_error_proto) as *mut JsObject,
        "SyntaxError" => P::as_ptr(&host.session().builtin_world().syntax_error_proto) as *mut JsObject,
        "URIError" => P::as_ptr(&host.session().builtin_world().uri_error_proto) as *mut JsObject,
        "EvalError" => P::as_ptr(&host.session().builtin_world().eval_error_proto) as *mut JsObject,
        _ => P::as_ptr(&host.session().builtin_world().error_proto) as *mut JsObject,
    };
    let obj = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());
    if !msg.is_empty() {
        let si_msg = sf.intern("message").0;
        let shape = sh.make_shape(EMPTY_SHAPE_ID, si_msg);
        let msg_val = host.new_string(msg);
        unsafe {
            (*obj).set_shape_id(shape);
            let pos = (*obj).push_prop(msg_val);
            // message 按规范为非枚举数据属性，避免泄漏进 Object.keys/for-in/JSON。
            (*obj).set_data_meta(pos, PropAttributes::new(true, false, true));
        }
    }
    // 标记 Error 家族标签：Object.prototype.toString 据此输出 `[object Error]`。
    unsafe {
        (*obj).type_tag = JsObject::OBJ_TYPE_ERROR;
    }
    JsValue::from_js_object(obj)
}

/// 创建一个带指定 message 的 TypeError 对象。
pub fn create_type_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "TypeError", msg)
}

/// 创建一个带指定 message 的普通 Error 对象。
pub fn create_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "Error", msg)
}

/// 错误文本的 kind 前缀表：`(kind 名, 前缀)`，供文本恢复与 `define` 通道共用。
/// `Error` 排在末位，避免在匹配更具体的子类前缀之前抢先命中。
const KIND_PREFIXES: [(&str, &str); 7] = [
    ("TypeError", "TypeError: "),
    ("ReferenceError", "ReferenceError: "),
    ("RangeError", "RangeError: "),
    ("SyntaxError", "SyntaxError: "),
    ("URIError", "URIError: "),
    ("EvalError", "EvalError: "),
    ("Error", "Error: "),
];

/// 拆分错误文本的 kind 前缀：`"RangeError: msg"` 返回 `Some(("RangeError", "msg"))`，
/// 无已知前缀返回 `None`。文本可带 `"uncaught "` 包装。
///
/// # 注意事项
/// - 仅在错误文本通道（kind 前缀约定）内使用；kind 名与创建入口的映射保持
///   单向，调用方不得据此改写原始异常值。
/// - 空消息错误的文本即裸 kind 名（如 `"uncaught RangeError"`，与
///   `String(new RangeError())` 序列化一致），按原文恢复 kind。
pub fn split_kinded(text: &str) -> Option<(&'static str, &str)> {
    let text = text.strip_prefix("uncaught ").unwrap_or(text);
    if let Some((kind, _)) = KIND_PREFIXES.iter().find(|(kind, _)| *kind == text) {
        return Some((*kind, ""));
    }
    KIND_PREFIXES
        .iter()
        .find_map(|(kind, prefix)| text.strip_prefix(prefix).map(|rest| (*kind, rest)))
}

/// 从错误文本（形如 `TypeError: msg` / `ReferenceError: msg`）解析类型前缀并创建
/// 对应 kind 的错误对象；无前缀时创建普通 Error。
///
/// # 使用场景
/// 异常传播链中错误对象被降级为文本（error_text 输出、`last_uncaught_value` 被
/// 嵌套覆盖后的兜底路径）时，据此恢复错误类型，避免全部塌缩成普通 Error。
pub fn create_from_text<H: VmHost>(host: &mut H, text: &str) -> JsValue {
    match split_kinded(text) {
        Some((kind, msg)) => create_kind_error(host, kind, msg),
        None => create_error(host, text.strip_prefix("uncaught ").unwrap_or(text)),
    }
}

/// 创建一个带指定 message 的 ReferenceError 对象。
pub fn create_reference_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "ReferenceError", msg)
}

/// 创建一个带指定 message 的 RangeError 对象。
pub fn create_range_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "RangeError", msg)
}

/// 从 `defineProperty` 通道的错误文本恢复异常对象。
///
/// 数组 length 的 `ArraySetLength` 非法值以 `"RangeError: "` 前缀标记 kind，
/// 强转期 `ToPrimitive`/`ToNumber` 失败以 `"TypeError: "` 前缀标记，两者均须保留
/// kind；其余 define 失败（非可配置收窄、不可扩展、accessor 冲突等）为无前缀
/// 文本，统一投影为 TypeError。
pub fn create_define_failure<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    match split_kinded(msg) {
        Some((kind, rest)) => create_kind_error(host, kind, rest),
        None => create_type_error(host, msg),
    }
}

/// 创建一个带指定 message 的 SyntaxError 对象。
pub fn create_syntax_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "SyntaxError", msg)
}

/// 创建一个带指定 message 的 URIError 对象。
pub fn create_uri_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "URIError", msg)
}

macro_rules! error_ctor {
    ($name:ident, $proto_field:ident) => {
        /// 对应 Error 子类（如 `TypeError`）的构造函数：接收第一个实参作为 message，
        /// 返回原型链指向对应 prototype 的 Error 对象。
        ///
        /// # 注意事项
        /// 无论以 `new` 还是普通函数调用（如 `TypeError.call(obj)`）都新建对象，
        /// 忽略调用方传入的 this——与规范构造器语义一致。
        pub fn $name<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
            let proto_ptr = P::as_ptr(&host.session().builtin_world().$proto_field) as *mut JsObject;
            let this = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
            set_own_message(host, this, args);
            NativeResult::Ok(JsValue::from_js_object(this))
        }
    };
}

error_ctor!(error_constructor, error_proto);
error_ctor!(type_error_constructor, type_error_proto);
error_ctor!(reference_error_constructor, reference_error_proto);
error_ctor!(range_error_constructor, range_error_proto);
error_ctor!(syntax_error_constructor, syntax_error_proto);
error_ctor!(uri_error_constructor, uri_error_proto);
error_ctor!(eval_error_constructor, eval_error_proto);

/// 在对象上追加一个非枚举数据属性（writable/configurable=true，enumerable=false），
/// 与 `CreateNonEnumerableDataPropertyOrThrow` 语义一致。
fn set_own_data_prop<H: VmHost>(host: &mut H, obj: *mut JsObject, key: &str, val: JsValue) {
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());
    let si = sf.intern(key).0;
    let new_shape = sh.make_shape(unsafe { (*obj).shape_id() }, si);
    unsafe {
        (*obj).set_shape_id(new_shape);
        let pos = (*obj).push_prop(val);
        (*obj).set_data_meta(pos, PropAttributes::new(true, false, true));
    }
}

/// `SuppressedError(error, suppressed, message)` 构造器：三参，length=3。
///
/// 以规范 CreateSuppressedError 语义建对象，三字段属性按 message → error
/// → suppressed 的顺序创建（该顺序有严格断言，实现不得交换），message 为
/// undefined 时省略；三者均非枚举数据属性。
///
/// # 边界与前提
/// - args[0]=this 被忽略：普通调用（无 new）同样新建对象，与既有 Error 子类型一致；
/// - args[1]=error、args[2]=suppressed 原样存储不转换；
/// - args[3]=message 走完整 ToString 强制转换：对象经 ToPrimitive（string hint），
///   Symbol 抛 TypeError；用户 toString 抛出的异常原样传播。
pub fn suppressed_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let proto_ptr = P::as_ptr(&host.session().builtin_world().suppressed_error_proto) as *mut JsObject;
    let obj = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));

    // message（args[3]）非 undefined 时做完整 ToString 转换并先写属性。
    if args.len() > 3 && !host.reg(args[3]).is_undefined() {
        let msg_str = match to_string_full(host.reg(args[3]), host) {
            Ok(s) => s,
            Err(_) => {
                // 对象 toString 抛出的用户异常经 last_uncaught_value 恢复后原样重新抛出；
                // Symbol 等不可转换场景由调用方构造 TypeError。
                if let Some(exc) = host.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(create_type_error(host, "Cannot convert value to a string"));
            }
        };
        let msg_val = host.new_string(&msg_str);
        set_own_data_prop(host, obj, "message", msg_val);
    }

    // error / suppressed 依次写非枚举数据属性（原值，不转换）。
    if args.len() > 1 {
        set_own_data_prop(host, obj, "error", host.reg(args[1]));
    }
    if args.len() > 2 {
        set_own_data_prop(host, obj, "suppressed", host.reg(args[2]));
    }
    unsafe {
        (*obj).type_tag = JsObject::OBJ_TYPE_ERROR;
    }
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// dispose 合并路径用：仅 error/suppressed 两个非枚举自有属性（无 message，
/// 对应规范 DisposeResources 分支的 CreateSuppressedError 调用）。
///
/// # 边界与前提
/// - 错误值/suppressed 值原样存储，不做任何转换；
/// - 返回对象 proto = suppressed_error_proto，标记 OBJ_TYPE_ERROR。
pub fn create_suppressed_error<H: VmHost>(host: &mut H, error_val: JsValue, suppressed_val: JsValue) -> JsValue {
    let proto_ptr = P::as_ptr(&host.session().builtin_world().suppressed_error_proto) as *mut JsObject;
    let obj = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    set_own_data_prop(host, obj, "error", error_val);
    set_own_data_prop(host, obj, "suppressed", suppressed_val);
    unsafe {
        (*obj).type_tag = JsObject::OBJ_TYPE_ERROR;
    }
    JsValue::from_js_object(obj)
}

/// `Error.prototype.toString`：按 `name: message` 拼接字符串；
/// 缺少 name/message 时按规范回退到 `"Error"` 或空串。
///
/// # 步骤
/// 1. 非对象 this 直接抛 TypeError（不做 ToObject 装箱）
/// 2. Get(name)：访问器 getter 触发，抛出的用户异常原样传播
/// 3. name 非 undefined 时 ToString（Symbol 抛 TypeError，用户转换异常传播）
/// 4. Get(message) 同 name；message 非 undefined 时 ToString
/// 5. name/message 任一为空串时只返回另一者，否则 `name: message`
///
/// # 边界与前提
/// - name/message 为 Symbol 时 ToString 转换抛 TypeError；
/// - getter 或 ToPrimitive 抛出的用户异常经 `take_uncaught_value` 原值恢复。
pub fn error_to_string<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let this_val = host.reg(args[0]);
    if !this_val.is_object() {
        let err = create_type_error(host, "Error.prototype.toString called on non-object");
        return NativeResult::Err(err);
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let si_name = sf.intern("name").0;
    let si_msg = sf.intern("message").0;

    let name_str = match host.ordinary_get(obj, si_name, this_val) {
        Ok(v) if v.is_undefined() => "Error".to_string(),
        Ok(v) => match to_string_full(v, host) {
            Ok(s) => s,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(host, &e)),
        },
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(host, &e)),
    };

    let msg_str = match host.ordinary_get(obj, si_msg, this_val) {
        Ok(v) if v.is_undefined() => String::new(),
        Ok(v) => match to_string_full(v, host) {
            Ok(s) => s,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(host, &e)),
        },
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(host, &e)),
    };

    let result = if name_str.is_empty() {
        msg_str
    } else if msg_str.is_empty() {
        name_str
    } else {
        format!("{}: {}", name_str, msg_str)
    };
    NativeResult::Ok(host.new_string(&result))
}

/// `Error.prototype.stack` getter：输出 `name: message` 头后附调用栈函数名列表。
pub fn error_stack_getter<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let this_val = host.reg(args[0]);
    let (name_str, msg_str) = if this_val.is_object() {
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        let sf = Arc::clone(host.kernel_core().perm_interner());
        let si_name = sf.intern("name").0;
        let si_msg = sf.intern("message").0;
        let n = host
            .resolve_property(obj, si_name)
            .and_then(|v| host.lookup_str(v))
            .unwrap_or_else(|| "Error".to_string());
        let m = host
            .resolve_property(obj, si_msg)
            .and_then(|v| host.lookup_str(v))
            .unwrap_or_default();
        (n, m)
    } else {
        ("Error".to_string(), String::new())
    };

    let header = oxide_runtime_api::format_error_message(&name_str, &msg_str);
    let mut result = header;
    let names = host.call_stack_function_names();
    for n in &names {
        result.push_str(&format!("\n    at {} (<unknown>:0:0)", n));
    }
    NativeResult::Ok(host.new_string(&result))
}
