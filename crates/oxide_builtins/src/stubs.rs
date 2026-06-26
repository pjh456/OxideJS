use oxide_runtime_api::{NativeResult, VmHost};

fn stub_error<H: VmHost>(vm: &mut H, name: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, &format!("{name} is not implemented")))
}

// Architecturally deferred — these features are excluded from the supported language subset by design.
pub fn proxy_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Proxy")
}
pub fn bigint_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "BigInt")
}
pub fn weakmap_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakMap")
}
pub fn weakset_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakSet")
}
pub fn weakref_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakRef")
}
pub fn finalization_registry_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "FinalizationRegistry")
}
pub fn shared_array_buffer_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "SharedArrayBuffer")
}
pub fn atomics_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Atomics")
}
