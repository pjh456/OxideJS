use oxide_types::value::JsValue;

use crate::native::NativeResult;
use crate::vm::Vm;

fn stub_error(vm: &mut Vm, name: &str) -> NativeResult {
    let msg = format!("TypeError: {} is not implemented", name);
    let si = vm.kernel_core().string_forge().intern(&msg).0;
    NativeResult::Err(JsValue::string(si, 0))
}

// Architecturally deferred — see 13.5-CONTEXT.md for rationale
pub fn proxy_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "Proxy") }
pub fn bigint_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "BigInt") }
pub fn weakmap_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "WeakMap") }
pub fn weakset_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "WeakSet") }
pub fn weakref_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "WeakRef") }
pub fn finalization_registry_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "FinalizationRegistry") }
pub fn shared_array_buffer_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "SharedArrayBuffer") }
pub fn atomics_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult { stub_error(vm, "Atomics") }
