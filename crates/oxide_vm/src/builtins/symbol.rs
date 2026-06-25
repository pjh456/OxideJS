use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::vm::Vm;
use oxide_runtime_api::NativeResult;

pub fn symbol_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
                        return NativeResult::Err(crate::builtins::error::create_type_error(
                            vm,
                            "Symbol is not a constructor",
                        ));
                    }
                }
            }
        }
    }

    let description = if args.len() > 1 {
        coercion::to_string(vm.reg(args[1]))
    } else {
        String::new()
    };

    vm.symbols.symbol_counter = vm.symbols.symbol_counter.wrapping_add(1);
    let idx = vm.symbols.symbol_descriptions.len() as u32;
    vm.symbols.symbol_descriptions.push(description);
    NativeResult::Ok(JsValue::symbol(idx))
}

pub fn symbol_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_symbol() {
        return NativeResult::Err(crate::builtins::error::create_type_error(
            vm,
            "Symbol.prototype.toString requires a Symbol",
        ));
    }

    let idx = this_val.as_symbol_index();
    let desc = vm.symbols.symbol_descriptions.get(idx as usize).cloned().unwrap_or_default();
    let result = format!("Symbol({})", desc);
    NativeResult::Ok(vm.new_string(&result))
}

pub fn symbol_for(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let key = if args.len() > 1 {
        coercion::to_string(vm.reg(args[1]))
    } else {
        "undefined".to_string()
    };

    if let Some(&idx) = vm.symbols.symbol_registry.get(&key) {
        return NativeResult::Ok(JsValue::symbol(idx));
    }

    let idx = vm.symbols.symbol_descriptions.len() as u32;
    vm.symbols.symbol_descriptions.push(key.clone());
    vm.symbols.symbol_registry.insert(key, idx);
    NativeResult::Ok(JsValue::symbol(idx))
}

pub fn symbol_key_for(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let sym = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !sym.is_symbol() {
        return NativeResult::Err(crate::builtins::error::create_type_error(vm, "is not a symbol"));
    }

    let idx = sym.as_symbol_index();
    let found = vm
        .symbols
        .symbol_registry
        .iter()
        .find(|(_, &registered_idx)| registered_idx == idx)
        .map(|(key, _)| key.clone());
    match found {
        Some(key) => NativeResult::Ok(vm.new_string(&key)),
        None => NativeResult::Ok(JsValue::undefined()),
    }
}
