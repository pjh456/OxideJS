use oxide_runtime_api::NativeResult;

pub type NativeFn = fn(&mut crate::vm::Vm, args: &[u8]) -> NativeResult;
