//! `oxide_runtime_api::VmHost` 委托实现：宿主接口各项委托到 `Vm` 同名固有方法。

use std::ffi::c_void;
use std::sync::Arc;

use oxide_kernel::KernelCore;
use oxide_runtime_api::{ListenerEntry, ProtoKind, ShapeNode};
use oxide_types::mem::P;
use oxide_types::object::{Cell, JsObject, PropAttributes};
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
    fn constructing_native(&self) -> bool {
        self.constructing_native
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
    fn number_to_string_cached(&mut self, d: f64) -> JsValue {
        Vm::number_to_string_cached(self, d)
    }
    fn new_bigint(&mut self, v: num_bigint::BigInt) -> JsValue {
        Vm::new_bigint(self, v)
    }
    fn bigint_value(&mut self, val: JsValue) -> &num_bigint::BigInt {
        Vm::bigint_value(self, val)
    }
    fn perm_intern(&self, s: &str) -> u32 {
        self.kernel_core.perm_interner().intern(s).0
    }
    fn perm_lookup(&self, si: u32) -> Option<&str> {
        self.kernel_core.perm_interner().lookup(si)
    }
    fn make_shape(&self, parent: u32, prop_si: u32) -> u32 {
        self.kernel_core.shape_forge().make_shape(parent, prop_si)
    }
    fn lookup_position(&self, shape_id: u32, prop_si: u32) -> Option<u32> {
        self.kernel_core.shape_forge().lookup_position(shape_id, prop_si)
    }
    fn get_shape(&self, id: u32) -> Option<ShapeNode> {
        self.kernel_core.shape_forge().get_shape(id).map(|s| ShapeNode {
            id: s.id,
            property_name: s.property_name,
            parent: s.parent,
            depth: s.depth,
        })
    }
    fn perm_interner_ptr(&self) -> *const c_void {
        Arc::as_ptr(self.kernel_core.perm_interner()) as *const c_void
    }
    fn shape_forge_ptr(&self) -> *const c_void {
        Arc::as_ptr(self.kernel_core.shape_forge()) as *const c_void
    }
    fn builtin_proto(&self, kind: ProtoKind) -> *mut JsObject {
        // session 守卫存活至方法返回：world 借用不跨语句长存。
        let session = self.session();
        let world = session.builtin_world();
        let p = match kind {
            ProtoKind::ObjectProto => &world.object_proto,
            ProtoKind::ArrayProto => &world.array_proto,
            ProtoKind::FunctionProto => &world.function_proto,
            ProtoKind::StringProto => &world.string_proto,
            ProtoKind::NumberProto => &world.number_proto,
            ProtoKind::BooleanProto => &world.boolean_proto,
            ProtoKind::SymbolProto => &world.symbol_proto,
            ProtoKind::BigIntProto => &world.bigint_proto,
            ProtoKind::ErrorProto => &world.error_proto,
            ProtoKind::TypeErrorProto => &world.type_error_proto,
            ProtoKind::ReferenceErrorProto => &world.reference_error_proto,
            ProtoKind::RangeErrorProto => &world.range_error_proto,
            ProtoKind::SyntaxErrorProto => &world.syntax_error_proto,
            ProtoKind::UriErrorProto => &world.uri_error_proto,
            ProtoKind::EvalErrorProto => &world.eval_error_proto,
            ProtoKind::SuppressedErrorProto => &world.suppressed_error_proto,
            ProtoKind::DateProto => &world.date_proto,
            ProtoKind::SetProto => &world.set_proto,
            ProtoKind::MapProto => &world.map_proto,
            ProtoKind::RegExpProto => &world.regexp_proto,
            ProtoKind::ArrayBufferProto => &world.array_buffer_proto,
            ProtoKind::SharedArrayBufferProto => &world.shared_array_buffer_proto,
            ProtoKind::DataViewProto => &world.data_view_proto,
            ProtoKind::TypedArrayProto => &world.typed_array_proto,
            ProtoKind::Int8ArrayProto => &world.int8array_proto,
            ProtoKind::Uint8ArrayProto => &world.uint8array_proto,
            ProtoKind::Uint8ClampedArrayProto => &world.uint8clampedarray_proto,
            ProtoKind::Int16ArrayProto => &world.int16array_proto,
            ProtoKind::Uint16ArrayProto => &world.uint16array_proto,
            ProtoKind::Int32ArrayProto => &world.int32array_proto,
            ProtoKind::Uint32ArrayProto => &world.uint32array_proto,
            ProtoKind::Float32ArrayProto => &world.float32array_proto,
            ProtoKind::Float64ArrayProto => &world.float64array_proto,
            ProtoKind::BigInt64ArrayProto => &world.bigint64array_proto,
            ProtoKind::BigUint64ArrayProto => &world.biguint64array_proto,
            ProtoKind::InstantProto => &world.instant_proto,
            ProtoKind::PlainDateProto => &world.plain_date_proto,
            ProtoKind::PlainTimeProto => &world.plain_time_proto,
            ProtoKind::DurationProto => &world.duration_proto,
            ProtoKind::ZonedDateTimeProto => &world.zoned_date_time_proto,
            ProtoKind::PlainDateTimeProto => &world.plain_date_time_proto,
            ProtoKind::PlainMonthDayProto => &world.plain_month_day_proto,
            ProtoKind::PlainYearMonthProto => &world.plain_year_month_proto,
            ProtoKind::IteratorProto => &world.iterator_proto,
            ProtoKind::ArrayIteratorProto => &world.array_iterator_proto,
            ProtoKind::MapIteratorProto => &world.map_iterator_proto,
            ProtoKind::SetIteratorProto => &world.set_iterator_proto,
            ProtoKind::StringIteratorProto => &world.string_iterator_proto,
            ProtoKind::RegExpStringIteratorProto => &world.regexp_string_iterator_proto,
            ProtoKind::IteratorHelperProto => &world.iterator_helper_proto,
            ProtoKind::DisposableStackProto => &world.disposable_stack_proto,
            ProtoKind::AsyncDisposableStackProto => &world.async_disposable_stack_proto,
            ProtoKind::RegExpConstructor => &world.regexp_constructor,
            ProtoKind::ArrayBufferConstructor => &world.array_buffer_constructor,
            ProtoKind::SharedArrayBufferConstructor => &world.shared_array_buffer_constructor,
            ProtoKind::Int8ArrayConstructor => &world.int8array_constructor,
            ProtoKind::Uint8ArrayConstructor => &world.uint8array_constructor,
            ProtoKind::Uint8ClampedArrayConstructor => &world.uint8clampedarray_constructor,
            ProtoKind::Int16ArrayConstructor => &world.int16array_constructor,
            ProtoKind::Uint16ArrayConstructor => &world.uint16array_constructor,
            ProtoKind::Int32ArrayConstructor => &world.int32array_constructor,
            ProtoKind::Uint32ArrayConstructor => &world.uint32array_constructor,
            ProtoKind::Float32ArrayConstructor => &world.float32array_constructor,
            ProtoKind::Float64ArrayConstructor => &world.float64array_constructor,
            ProtoKind::BigInt64ArrayConstructor => &world.bigint64array_constructor,
            ProtoKind::BigUint64ArrayConstructor => &world.biguint64array_constructor,
            ProtoKind::MessagePortProto => &world.message_port_proto,
            ProtoKind::BroadcastChannelProto => &world.broadcast_channel_proto,
            ProtoKind::EventProto => &world.event_proto,
            ProtoKind::MessageEventProto => &world.message_event_proto,
            ProtoKind::ErrorEventProto => &world.error_event_proto,
            ProtoKind::CustomEventProto => &world.custom_event_proto,
        };
        P::as_ptr(p) as *mut JsObject
    }
    fn string_default_iterator(&self) -> *const JsObject {
        let session = self.session();
        session.builtin_world().string_default_iterator.get()
    }
    fn global_object(&self) -> P<JsObject> {
        let session = self.session();
        session.global_object().clone()
    }
    fn kernel_core(&self) -> Arc<KernelCore> {
        Arc::clone(self.kernel_core())
    }
    fn pc(&self) -> usize {
        self.pc
    }
    fn take_uncaught_value(&mut self) -> Option<JsValue> {
        self.last_uncaught_value.take()
    }
    fn restore_uncaught_value(&mut self, value: Option<JsValue>) {
        self.last_uncaught_value = value;
    }
    fn take_pending_length_exception(&mut self) -> Option<JsValue> {
        self.pending_length_exception.take()
    }
    fn clear_uncaught_value(&mut self) {
        self.last_uncaught_value = None;
    }
    fn move_uncaught_to_pending_length(&mut self) {
        self.pending_length_exception = self.last_uncaught_value.take();
    }
    fn property_key_si(&mut self, val: JsValue) -> u32 {
        // 对象键转换（to_string_full）失败时降级为空键：本 trait 路径供不传播
        // 键转换异常的内置站点（Object defineProperty/fromEntries/hasOwn、
        // 数值串内部键等）使用；按规范须传播异常的站点用返回 Result 的
        // `to_property_key_si`，转换异常原样保留。
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
    fn has_property(&mut self, obj: &JsObject, prop_name_si: u32) -> Result<bool, String> {
        self.has_property(obj, prop_name_si)
    }
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        self.get_own_property_slot(obj, prop_name_si)
    }
    fn arguments_mapping_alive(&self, state: &oxide_types::arguments_map::ArgumentsMapState) -> bool {
        self.arguments_mapping_alive(state)
    }
    fn is_top_frame(&self, frame_depth: u32) -> bool {
        self.is_top_frame(frame_depth)
    }
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String> {
        self.ordinary_get(obj, prop_name_si, receiver)
    }
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        // builtin 调用边界：写失败返回格式化 Err，不就地展开（见 ordinary_set_builtin）。
        self.ordinary_set_builtin(obj, prop_name_si, val, receiver, strict)
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
    fn ensure_deferred_ns_evaluation(&mut self, obj: &JsObject, key_si: Option<u32>) -> Result<(), String> {
        // 非 deferred module namespace 直接 no-op（热路径零成本）。
        if !obj.is_module_namespace() || !obj.is_deferred() {
            return Ok(());
        }
        // symbol-like 键（Symbol 或 deferred 的 "then"）走 Ordinary* 路径，不触发。
        if let Some(key) = key_si {
            if oxide_types::private_key::is_symbol_key(key) {
                return Ok(());
            }
            if key == self.perm_intern("then") {
                return Ok(());
            }
        }
        // 读状态盒（经 native_data 裸指针，Box 堆分配 GC 不搬移，指针稳定）。
        let Some(state) = oxide_builtins::module::deferred_state_mut(obj) else {
            return Ok(());
        };
        if state.evaluated {
            // 已求值：失败缓存原值重抛（保错误对象身份），成功 no-op。
            if let Some(err) = state.error {
                // 还原 uncaught 侧通道为缓存的原错误值，调用方（[[Get]] 等）
                // 经 raise_call_error 原值重抛，保 sameValue 身份。
                self.last_uncaught_value = Some(err);
                return Err(self.error_text(err));
            }
            return Ok(());
        }
        // cyclic 守卫：[[Module]] 为 undefined 哨兵（自导入/祖先 defer）时，
        // 标记在场即求值中 → TypeError；标记已清（祖先已求值完毕）按已求值处理。
        if state.module.is_undefined() {
            if self.evaluating_module.is_some() {
                return Err(self.error_message_text("TypeError", "cyclic module evaluation"));
            }
            state.evaluated = true;
            return Ok(());
        }
        // [[Module]] 为函数：标记等于本函数即自求值中（安全网）→ TypeError。
        if self.evaluating_module == Some(state.module) {
            return Err(self.error_message_text("TypeError", "cyclic module evaluation"));
        }
        // EvaluateSync：调依赖模块函数，缓存结果命名空间或失败错误。
        match self.call_function_sync(state.module, JsValue::undefined(), &[]) {
            Ok(ns) => {
                state.namespace = ns;
                state.evaluated = true;
            }
            Err(text) => {
                // 原错误值经 uncaught 侧通道取走存入状态盒，并还原侧通道供
                // 调用方（[[Get]] 等）原值重抛。
                let exc = self.last_uncaught_value.take();
                state.error = exc;
                state.evaluated = true;
                self.last_uncaught_value = exc;
                return Err(text);
            }
        }
        Ok(())
    }
    fn evaluating_module(&self) -> Option<JsValue> {
        self.evaluating_module
    }
    fn set_evaluating_module(&mut self, val: Option<JsValue>) {
        self.evaluating_module = val;
    }
    fn construct_ctor(&mut self, ctor: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        Vm::construct_ctor(self, ctor, args)
    }
    fn construct_ctor_nt(&mut self, ctor: JsValue, new_target: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        Vm::construct_with(self, ctor, new_target, args)
    }
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String> {
        self.checked_object_ptr(val, error_msg)
    }
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_type_error(msg)
    }
    fn raise_captured(&mut self, exc: JsValue) -> Result<(), String> {
        self.raise_captured(exc)
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
    fn function_is_strict(&self, obj: &JsObject) -> bool {
        // native 函数（sub_module_index == 0）无子模块，callee_module 返回 None，
        // 按严格处理（受限访问器对其抛错）。
        self.callee_module(obj).map(|m| m.is_strict).unwrap_or(true)
    }
    fn function_is_restricted(&self, obj: &JsObject) -> bool {
        // 严格函数对象，或 [[Prototype]] 为三个动态函数原型之一（生成器 /
        // 异步 / 异步生成器函数）：caller/arguments 访问一律受限。
        if self.function_is_strict(obj) {
            return true;
        }
        let proto = obj.proto();
        if !proto.is_object() {
            return false;
        }
        let p = proto.as_js_object_ptr() as *const JsObject;
        std::ptr::eq(p, self.realm.generator_function_proto.borrow().as_ptr())
            || std::ptr::eq(p, self.realm.async_function_proto.borrow().as_ptr())
            || std::ptr::eq(p, self.realm.async_generator_function_proto.borrow().as_ptr())
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
    fn module_frame_cell(&self, cell_idx: u32) -> Option<*mut Cell> {
        self.cell_stack
            .last()
            .and_then(|cells| cells.get(cell_idx as usize).copied())
            .filter(|p| !p.is_null())
    }
    fn create_dynamic_function(
        &mut self, params: &[String], body: &str, is_generator: bool, is_async: bool,
    ) -> Result<JsValue, String> {
        self.create_dynamic_function(params, body, is_generator, is_async)
    }
    fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        self.create_dynamic_script(code)
    }
    fn realm_id(&self) -> u32 {
        self.realm_id()
    }
    fn symbol_intern(&mut self, desc: Option<String>) -> u32 {
        self.realm.symbols.borrow_mut().intern(desc)
    }
    fn symbol_description(&self, idx: u32) -> Option<String> {
        self.realm.symbols.borrow().description(idx).map(|s| s.to_string())
    }
    fn symbol_lookup_global(&self, key: &str) -> Option<u32> {
        self.realm.symbols.borrow().lookup_global(key)
    }
    fn symbol_register_global(&mut self, key: String, idx: u32) {
        self.realm.symbols.borrow_mut().register_global(key, idx)
    }
    fn symbol_key_for_id(&self, idx: u32) -> Option<String> {
        self.realm.symbols.borrow().key_for_id(idx)
    }
    fn atomics_new_waiter_promise(&mut self) -> JsValue {
        self.atomics_new_waiter_promise()
    }
    fn atomics_register_waiter(&mut self, buffer: *mut JsObject, offset: usize, promise: JsValue) {
        self.atomics_register_waiter(buffer, offset, promise)
    }
    fn atomics_wake_waiters(&mut self, buffer: *mut JsObject, offset: usize, count: f64) -> usize {
        self.atomics_wake_waiters(buffer, offset, count)
    }
    fn bc_register(&self, name: &str, port: *mut JsObject) {
        self.realm
            .gc
            .borrow_mut()
            .broadcast_channels
            .entry(name.to_string())
            .or_default()
            .push(port);
    }
    fn bc_unregister(&self, name: &str, port: *mut JsObject) {
        if let Some(v) = self.realm.gc.borrow_mut().broadcast_channels.get_mut(name) {
            v.retain(|p| *p != port);
        }
    }
    fn bc_lookup(&self, name: &str) -> Vec<*mut JsObject> {
        self.realm.gc.borrow().broadcast_channels.get(name).cloned().unwrap_or_default()
    }
    fn et_register(&self, target: *mut JsObject, entry: ListenerEntry) {
        self.realm
            .gc
            .borrow_mut()
            .event_targets
            .entry(target)
            .or_default()
            .listeners
            .push(entry);
    }
    fn et_unregister(&self, target: *mut JsObject, type_si: u32, callback: JsValue, capture: bool) {
        let mut gc = self.realm.gc.borrow_mut();
        let should_remove = gc.event_targets.get_mut(&target).is_some_and(|state| {
            state.listeners.retain(|e| !(e.type_si == type_si && e.capture == capture && e.callback == callback));
            state.listeners.is_empty()
        });
        if should_remove {
            gc.event_targets.remove(&target);
        }
    }
    fn et_lookup(&self, target: *mut JsObject) -> Vec<ListenerEntry> {
        self.realm.gc.borrow().event_targets.get(&target).map(|s| s.listeners.clone()).unwrap_or_default()
    }
}
