use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

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
                    let sp = vm.kernel().builtin_world().symbol_proto.as_ptr() as *mut JsObject;
                    if std::ptr::eq(proto_ptr, sp) {
                        return Err(JsValue::string(
                            vm.kernel()
                                .string_forge()
                                .intern("TypeError: Symbol is not a constructor")
                                .0,
                            0,
                        ));
                    }
                }
            }
        }
    }

    let description = if args.len() > 1 {
        coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[1]))
    } else {
        String::new()
    };

    vm.symbol_counter = vm.symbol_counter.wrapping_add(1);
    let idx = vm.symbol_descriptions.len() as u32;
    vm.symbol_descriptions.push(description);
    Ok(JsValue::symbol(idx))
}

pub fn symbol_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_symbol() {
        return Err(JsValue::string(
            vm.kernel()
                .string_forge()
                .intern("TypeError: Symbol.prototype.toString requires a Symbol")
                .0,
            0,
        ));
    }

    let idx = this_val.as_symbol_index();
    let desc = vm
        .symbol_descriptions
        .get(idx as usize)
        .cloned()
        .unwrap_or_default();
    let result = format!("Symbol({})", desc);
    let si = vm.kernel().string_forge().intern(&result).0;
    Ok(JsValue::string(si, 0))
}
