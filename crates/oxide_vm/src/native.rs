use oxide_types::value::JsValue;

pub type NativeResult = Result<JsValue, JsValue>;

pub type NativeFn = fn(&mut crate::vm::Vm, args: &[u8]) -> NativeResult;
