use crate::native::NativeResult;
use crate::vm::Vm;

fn stub_error(vm: &mut Vm, name: &str) -> NativeResult {
    NativeResult::Err(crate::builtins::error::create_type_error(vm, &format!("{name} is not implemented")))
}

// Architecturally deferred — these features are excluded from the supported language subset by design.
pub fn proxy_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Proxy")
}
pub fn bigint_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "BigInt")
}
pub fn weakmap_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakMap")
}
pub fn weakset_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakSet")
}
pub fn weakref_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakRef")
}
pub fn finalization_registry_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "FinalizationRegistry")
}
pub fn shared_array_buffer_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "SharedArrayBuffer")
}
pub fn atomics_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Atomics")
}
