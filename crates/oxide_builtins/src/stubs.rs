use oxide_runtime_api::{NativeResult, VmHost};

fn stub_error<H: VmHost>(vm: &mut H, name: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, &format!("{name} is not implemented")))
}

// 架构性延后：这些特性按设计排除在受支持语言子集之外。
/// `Proxy` 全局占位：始终抛 TypeError。Proxy 按设计延后实现。
pub fn proxy_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Proxy")
}
/// `BigInt` 全局占位：始终抛 TypeError。BigInt 按设计延后实现。
pub fn bigint_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "BigInt")
}
/// `WeakMap` 全局占位：始终抛 TypeError。WeakMap 按设计延后实现。
pub fn weakmap_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakMap")
}
/// `WeakSet` 全局占位：始终抛 TypeError。WeakSet 按设计延后实现。
pub fn weakset_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakSet")
}
/// `WeakRef` 全局占位：始终抛 TypeError。WeakRef 按设计延后实现。
pub fn weakref_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "WeakRef")
}
/// `FinalizationRegistry` 全局占位：始终抛 TypeError。按设计延后实现。
pub fn finalization_registry_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "FinalizationRegistry")
}
/// `SharedArrayBuffer` 全局占位：始终抛 TypeError。按设计延后实现。
pub fn shared_array_buffer_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "SharedArrayBuffer")
}
/// `Atomics` 全局占位：始终抛 TypeError。按设计延后实现。
pub fn atomics_stub<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    stub_error(vm, "Atomics")
}
