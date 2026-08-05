use oxide_runtime_api::{NativeResult, VmHost};

fn stub_error<H: VmHost>(vm: &mut H, name: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, &format!("{name} is not implemented")))
}

// Architecturally deferred — these features are excluded from the supported language subset by design.
/// Stub for the `Proxy` global. Always throws a TypeError; Proxy is deferred by design.
pub fn proxy_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Proxy")
}
/// Stub for the `BigInt` global. Always throws a TypeError; BigInt is deferred by design.
pub fn bigint_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "BigInt")
}
/// Stub for the `WeakMap` global. Always throws a TypeError; WeakMap is deferred by design.
pub fn weakmap_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakMap")
}
/// Stub for the `WeakSet` global. Always throws a TypeError; WeakSet is deferred by design.
pub fn weakset_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakSet")
}
/// Stub for the `WeakRef` global. Always throws a TypeError; WeakRef is deferred by design.
pub fn weakref_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakRef")
}
/// Stub for the `FinalizationRegistry` global. Always throws a TypeError; deferred by design.
pub fn finalization_registry_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "FinalizationRegistry")
}
/// Stub for the `SharedArrayBuffer` global. Always throws a TypeError; deferred by design.
pub fn shared_array_buffer_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "SharedArrayBuffer")
}
/// Stub for the `Atomics` global. Always throws a TypeError; deferred by design.
pub fn atomics_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Atomics")
}
