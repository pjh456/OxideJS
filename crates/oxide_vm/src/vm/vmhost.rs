//! `oxide_runtime_api::VmHost` 委托实现：宿主接口各项委托到 `Vm` 同名固有方法。

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::Epoch;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use super::Vm;

impl oxide_runtime_api::VmHost for Vm {
    fn reg(&self, idx: u8) -> JsValue {
        self.reg(idx)
    }
    fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.set_reg(idx, val);
    }
    fn native_overflow_count(&self) -> usize {
        self.native_overflow_count
    }
    fn native_overflow_at(&self, i: usize) -> JsValue {
        self.spill_stack[self.native_overflow_base + i]
    }
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        self.alloc_object(obj)
    }
    fn new_string(&mut self, s: &str) -> JsValue {
        self.new_string(s)
    }
    fn new_string_owned(&mut self, s: String) -> JsValue {
        Vm::new_string_owned(self, s)
    }
    fn new_bigint(&mut self, v: num_bigint::BigInt) -> JsValue {
        Vm::new_bigint(self, v)
    }
    fn bigint_value(&mut self, val: JsValue) -> &num_bigint::BigInt {
        Vm::bigint_value(self, val)
    }
    fn kernel_core(&self) -> &Arc<KernelCore> {
        self.kernel_core()
    }
    fn session(&self) -> &KernelSession {
        self.session()
    }
    fn epoch(&self) -> &Epoch {
        self.epoch()
    }
    fn take_uncaught_value(&mut self) -> Option<JsValue> {
        self.last_uncaught_value.take()
    }
    fn restore_uncaught_value(&mut self, value: Option<JsValue>) {
        self.last_uncaught_value = value;
    }
    fn property_key_si(&mut self, val: JsValue) -> u32 {
        // Object-key conversion (to_string_full) failures degrade to the empty key here:
        // this trait path is used by Reflect/Object builtins; computed property access
        // uses the inherent Result-returning version to preserve the full exception.
        self.property_key_si(val)
            .unwrap_or_else(|_| self.kernel_core.perm_interner().intern("").0)
    }
    fn to_property_key_si(&mut self, val: JsValue) -> Result<u32, String> {
        // ToPropertyKey 完整路径：转换异常（对象 ToPrimitive 抛错 / Symbol 处理）
        // 原样返回，供需传播异常的 builtins（groupBy / __defineGetter__ 等）使用。
        self.property_key_si(val)
    }
    fn string_key_si(&mut self, s: &str) -> u32 {
        // 与运行时字符串键同口径：含 FFFD 的键文本按 encode_key 形态入键空间
        // （FFFD 键的 JSON.parse 键与字面量/拼接构造的运行时键身份一致）。
        self.string_key_text(s)
    }
    fn string_units(&self, val: JsValue) -> std::borrow::Cow<'_, [u16]> {
        // SAFETY: 调用方保证 val 为字符串值。perm 串由内核持有永不释放；session
        // 串仅经 &mut self 路径（new_string/GC）释放，&self 借用期间编译器强制
        // 不存在 &mut 存续，字符串不会在借用期内回收。
        unsafe { (*val.as_string_ptr()).units() }
    }
    fn new_string_units(&mut self, units: &[u16]) -> JsValue {
        Vm::new_string_units(self, units)
    }
    fn new_string_units_owned(&mut self, units: Vec<u16>) -> JsValue {
        Vm::new_string_units_owned(self, units)
    }
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        self.resolve_property(obj, prop_name_si)
    }
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        self.get_own_property_slot(obj, prop_name_si)
    }
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String> {
        self.ordinary_get(obj, prop_name_si, receiver)
    }
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        self.ordinary_set(obj, prop_name_si, val, receiver, strict)
    }
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        self.define_data_property(obj, prop_name_si, val, attributes)
    }
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        self.define_accessor_property(obj, prop_name_si, get, set, attributes)
    }
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        self.set_or_create_prop_value(obj, prop_name_si, val)
    }
    fn sync_global_builtin_mirror(&mut self, obj: &JsObject, key_si: u32, val: JsValue) {
        self.sync_global_builtin_mirror(obj, key_si, val)
    }
    fn lookup_str(&self, val: JsValue) -> Option<String> {
        self.lookup_str(val)
    }
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        self.coerce_primitive_bounded(value, prefer_string)
    }
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        self.coerce_number_bounded(value)
    }
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String> {
        self.call_function_sync(callee, receiver, args)
    }
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String> {
        self.checked_object_ptr(val, error_msg)
    }
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_type_error(msg)
    }
    fn error_message_text(&self, kind: &str, msg: &str) -> String {
        self.error_message_text(kind, msg)
    }
    fn call_stack_function_names(&self) -> Vec<String> {
        self.frames
            .iter()
            .rev()
            .map(|f| {
                self.kernel_core
                    .perm_interner()
                    .lookup(f.function_name)
                    .unwrap_or("<anonymous>")
                    .to_string()
            })
            .collect()
    }
    fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue {
        self.promote_if_needed_for_write_ptr(target_ptr, value)
    }
    fn step_rng(&mut self) {
        self.step_rng()
    }
    fn math_rng_value(&self) -> f64 {
        self.math_rng_value()
    }
    fn sub_module_function_name(&self, gen: u32, sub_idx: u16) -> String {
        self.tables
            .get(&gen)
            .and_then(|t| t.modules.get(sub_idx as usize))
            .and_then(|m| m.function_name.clone())
            .unwrap_or_default()
    }
    fn create_dynamic_function(&mut self, params: &[String], body: &str) -> Result<JsValue, String> {
        self.create_dynamic_function(params, body)
    }
    fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        self.create_dynamic_script(code)
    }
    fn symbol_intern(&mut self, desc: Option<String>) -> u32 {
        self.symbols.intern(desc)
    }
    fn symbol_description(&self, idx: u32) -> Option<&str> {
        self.symbols.description(idx)
    }
    fn symbol_lookup_global(&self, key: &str) -> Option<u32> {
        self.symbols.lookup_global(key)
    }
    fn symbol_register_global(&mut self, key: String, idx: u32) {
        self.symbols.register_global(key, idx)
    }
    fn symbol_key_for_id(&self, idx: u32) -> Option<String> {
        self.symbols.key_for_id(idx)
    }
}
