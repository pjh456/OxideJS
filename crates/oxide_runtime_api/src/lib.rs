//! `oxide_runtime_api` — the abstract interface between builtins and the VM.
//!
//! Builtins are written generically against the [`VmHost`] trait
//! (`fn xxx<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult`); `Vm`
//! implements `VmHost`. This breaks what would otherwise be a circular
//! dependency between the builtins crate and `oxide_vm`:
//!
//! `oxide_types ← oxide_kernel ← oxide_runtime_api ← oxide_builtins ← oxide_vm`
//!
//! The trait is GENERIC-friendly, not object-safe: monomorphizing `H = Vm`
//! inlines every `host.*()` call, so there is zero runtime overhead versus
//! builtins living inside `oxide_vm`.

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::Epoch;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// Return value of every builtin native function.
pub enum NativeResult {
    Ok(JsValue),
    Err(JsValue),
    TailCall { callee: JsValue, this: JsValue, args: Vec<JsValue> },
}

impl NativeResult {
    pub fn ok(val: JsValue) -> Self {
        Self::Ok(val)
    }

    pub fn err(val: JsValue) -> Self {
        Self::Err(val)
    }

    pub fn unwrap(self) -> JsValue {
        match self {
            Self::Ok(val) => val,
            Self::Err(_) => panic!("called `NativeResult::unwrap()` on an `Err` value"),
            Self::TailCall { .. } => panic!("called `NativeResult::unwrap()` on a `TailCall` value"),
        }
    }

    pub fn map_err<E, F>(self, op: F) -> Result<JsValue, E>
    where
        F: FnOnce(JsValue) -> E,
    {
        match self {
            Self::Ok(val) => Ok(val),
            Self::Err(err) => Err(op(err)),
            Self::TailCall { .. } => panic!("TailCall cannot be converted to Result"),
        }
    }
}

/// The set of `Vm` capabilities that builtins depend on.
///
/// Signatures are byte-for-byte copies of the corresponding inherent methods on
/// `Vm`; `impl VmHost for Vm` delegates to them. The trait is intentionally
/// flat and not object-safe — builtins always take `&mut impl VmHost`.
pub trait VmHost {
    // Register access
    fn reg(&self, idx: u8) -> JsValue;
    fn set_reg(&mut self, idx: u8, val: JsValue);

    // Object allocation / string creation
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject;
    fn new_string(&mut self, s: &str) -> JsValue;

    // Kernel accessors
    fn kernel_core(&self) -> &Arc<KernelCore>;
    fn session(&self) -> &KernelSession;
    fn epoch(&self) -> &Epoch;

    // Property resolution
    fn property_key_si(&mut self, val: JsValue) -> u32;
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue>;
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32>;

    // Property access
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String>;
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String>;

    // Property definition
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue);

    // Lookup / coercion
    fn lookup_str(&self, val: JsValue) -> Option<String>;
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String>;
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String>;

    // Call infrastructure
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String>;

    // Error handling
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String>;
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String>;
    fn error_message_text(&self, kind: &str, msg: &str) -> String;
}
