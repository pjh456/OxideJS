use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

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

    let description = if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        String::new()
    };

    let idx = vm.symbol_intern(description);
    NativeResult::Ok(JsValue::symbol(idx))
}

pub fn symbol_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Symbol.prototype.toString requires a Symbol"));
    }

    let idx = this_val.as_symbol_index();
    let desc = vm.symbol_description(idx).unwrap_or("").to_string();
    let result = format!("Symbol({})", desc);
    NativeResult::Ok(vm.new_string(&result))
}

pub fn symbol_for<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let key = if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        "undefined".to_string()
    };

    if let Some(idx) = vm.symbol_lookup_global(&key) {
        return NativeResult::Ok(JsValue::symbol(idx));
    }

    let idx = vm.symbol_intern(key.clone());
    vm.symbol_register_global(key, idx);
    NativeResult::Ok(JsValue::symbol(idx))
}

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
