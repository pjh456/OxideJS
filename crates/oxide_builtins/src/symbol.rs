use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

/// JS `Symbol()` 构造逻辑：以可选 description 创建一个新的唯一 Symbol。
/// 当以 new 语义调用（this 原型链指向 Symbol.prototype）时抛 TypeError，与规范一致。
pub fn symbol_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            let proto = obj.proto();
            if proto.is_object() {
                let proto_ptr = proto.as_js_object_ptr();
                if !proto_ptr.is_null() {
                    let sp = vm.session().builtin_world().symbol_proto.as_ptr() as *mut JsObject;
                    if std::ptr::eq(proto_ptr, sp) {
                        return NativeResult::Err(crate::error::create_type_error(vm, "Symbol is not a constructor"));
                    }
                }
            }
        }
    }

    // 描述符按规范 ToString(description)：对象经 ToPrimitive(string hint) 强制转换，
    // Symbol 抛 TypeError，对象方法抛出的原始异常原样传播。
    let description = if args.len() > 1 {
        match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
            Ok(s) => s,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    } else {
        String::new()
    };

    let idx = vm.symbol_intern(Some(description));
    NativeResult::Ok(JsValue::symbol(idx))
}

/// `Symbol.prototype.toString`：返回 `Symbol(description)` 形式字符串。
/// this 必须是 Symbol，否则抛 TypeError。
/// thisSymbolValue：Symbol 原样返回；包装对象解盒（prop 0 存原始 Symbol）；其它类型 TypeError。
fn this_symbol_value<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<JsValue, JsValue> {
    if this_val.is_symbol() {
        return Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.proto().is_object() {
                let proto_ptr = obj.proto().as_js_object_ptr();
                let symbol_proto =
                    vm.session().builtin_world().symbol_proto.as_ptr() as *mut oxide_types::object::JsObject;
                if !proto_ptr.is_null() && std::ptr::eq(proto_ptr, symbol_proto) {
                    let v = obj.get_prop_at(0);
                    if v.is_symbol() {
                        return Ok(v);
                    }
                }
            }
        }
    }
    Err(crate::error::create_type_error(
        vm,
        "Symbol.prototype method called on incompatible receiver",
    ))
}

/// `Symbol.prototype.toString`：返回 `Symbol(description)` 形式字符串。
/// this 必须是 Symbol 或 Symbol 包装对象，否则抛 TypeError。
pub fn symbol_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    let sym = match this_symbol_value(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let idx = sym.as_symbol_index();
    let desc = vm.symbol_description(idx).unwrap_or("").to_string();
    let result = format!("Symbol({})", desc);
    NativeResult::Ok(vm.new_string(&result))
}

/// `Symbol.prototype.valueOf`：Symbol 原样返回；包装对象解盒返回其原始 Symbol。
pub fn symbol_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    match this_symbol_value(vm, vm.reg(args[0])) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// `Symbol.for(key)`：在全局 symbol 注册表中查找并返回同 key 的 Symbol；
/// 不存在则新建并登记。同 key 的 Symbol 在全局唯一。
pub fn symbol_for<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // Symbol.for(key) 按规范 ToString(key)：对象经完整强制转换，异常原样传播。
    let key = if args.len() > 1 {
        match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
            Ok(s) => s,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    } else {
        "undefined".to_string()
    };

    if let Some(idx) = vm.symbol_lookup_global(&key) {
        return NativeResult::Ok(JsValue::symbol(idx));
    }

    let idx = vm.symbol_intern(Some(key.clone()));
    vm.symbol_register_global(key, idx);
    NativeResult::Ok(JsValue::symbol(idx))
}

/// `Symbol.keyFor(sym)`：返回全局注册表中该 Symbol 的 key；未登记返回 undefined。
/// 参数非 Symbol 抛 TypeError。
pub fn symbol_key_for<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let sym = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !sym.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "is not a symbol"));
    }

    let idx = sym.as_symbol_index();
    match vm.symbol_key_for_id(idx) {
        Some(key) => NativeResult::Ok(vm.new_string(&key)),
        None => NativeResult::Ok(JsValue::undefined()),
    }
}
