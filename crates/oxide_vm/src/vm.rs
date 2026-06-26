#![allow(clippy::arc_with_non_send_sync)]

use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use smallvec::SmallVec;

pub use crate::bindings::init_kernel_builtins;
use crate::native::NativeFn;
use crate::session_gc::SessionGc;
use crate::vm_state::{GcState, IterState, ProfilingState, SymbolState};
use crate::{vm_debug, vm_error, vm_trace, vm_warn};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_runtime_api as coercion;
use oxide_runtime_api::NativeResult;
use oxide_types::error::{JsError, JsErrorKind};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

pub(crate) const MAX_PROTO_CHAIN_DEPTH: usize = 1024;

/// Convert a `NativeFnPtr` to a callable `NativeFn`.
///
/// # Safety
/// The pointer stored in `ptr` must have been created from a valid `NativeFn` fn-item.
/// This is the single point in the codebase where `NativeFnPtr → NativeFn` coercion happens.
#[inline(always)]
pub(crate) unsafe fn native_fn_ptr_to_fn(ptr: NativeFnPtr) -> NativeFn {
    std::mem::transmute::<*const (), NativeFn>(ptr.as_ptr())
}

fn js_error_kind(kind: &'static str) -> JsErrorKind {
    match kind {
        "TypeError" => JsErrorKind::TypeError,
        "RangeError" => JsErrorKind::RangeError,
        "ReferenceError" => JsErrorKind::ReferenceError,
        "SyntaxError" => JsErrorKind::SyntaxError,
        "URIError" => JsErrorKind::URIError,
        "EvalError" => JsErrorKind::EvalError,
        _ => JsErrorKind::Error,
    }
}

fn js_error_kind_name(kind: JsErrorKind) -> &'static str {
    match kind {
        JsErrorKind::TypeError => "TypeError",
        JsErrorKind::RangeError => "RangeError",
        JsErrorKind::ReferenceError => "ReferenceError",
        JsErrorKind::SyntaxError => "SyntaxError",
        JsErrorKind::Error => "Error",
        JsErrorKind::URIError => "URIError",
        JsErrorKind::EvalError => "EvalError",
    }
}

pub(crate) fn format_error_message(kind: &str, msg: &str) -> String {
    if msg.is_empty() || msg == kind || msg.starts_with(&format!("{kind}:")) {
        msg.to_string()
    } else {
        format!("{kind}: {msg}")
    }
}

#[allow(unused_macros)]
macro_rules! throw_err {
    ($self:ident, $kind:ident, $msg:expr) => {{
        match $self.raise_error_kind(stringify!($kind), $msg) {
            Ok(()) => continue,
            Err(e) => return Err(e),
        }
    }};
}

macro_rules! binary_arith {
    ($self:ident, $a:expr, $b:expr, $rd:expr, $op:tt) => {{
        let l = $self.coerce_number_bounded($self.regs[$a])?;
        let r = $self.coerce_number_bounded($self.regs[$b])?;
        $self.regs[$rd] = JsValue::float(l $op r);
    }}
}

#[derive(Debug, Clone, Copy)]
pub enum FrameContinuation {
    None,
    AccessorGet { target_reg: u8 },
    AccessorSet,
}

pub struct CallFrame {
    pub return_addr: usize,
    pub function_name: u32,
    pub caller_reg_limit: u8,
    pub saved_reg_offset: u32,
    pub saved_this: JsValue,
    pub saved_new_target: JsValue,
    pub callee: JsValue,
    pub construct_result_reg: Option<u8>,
    pub constructed_this: Option<JsValue>,
    pub is_derived_constructor: bool,
    pub continuation: FrameContinuation,
}

pub struct ForInIter<'bump> {
    pub keys: bumpalo::collections::Vec<'bump, JsValue>,
    pub index: usize,
}

pub struct TryHandler {
    pub catch_pc: Option<usize>,
    pub finally_pc: Option<usize>,
    pub frame_depth: usize,
}

/// Heap-allocated snapshot used by `call_bytecode_function_inline`.
/// Keeping it on the heap prevents Rust stack overflow when JS code
/// chains multiple sync bytecode calls (e.g. sort comparator, accessor).
pub(crate) struct InlineSyncState {
    pub(crate) regs: Box<[JsValue; 256]>,
    pub(crate) pc: usize,
    pub(crate) bytecode: Vec<opcode::Instr>,
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) active_reg_limit: u8,
    pub(crate) root_reg_limit: u8,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) exception_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<JsValue>,
    pub(crate) last_for_of_result: JsValue,
    pub(crate) saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    pub(crate) save_stack: Vec<JsValue>,
}

pub struct Vm {
    pub(crate) regs: [JsValue; 256],
    pub(crate) pc: usize,
    pub(crate) bytecode: Vec<opcode::Instr>,
    /// Per-run convert-once immutables cache. Index 0 = top module, sub_idx+1 = sub_modules[sub_idx].
    /// Each `OnceLock` holds that module's constants converted to `JsValue`s exactly once this run.
    /// Rebuilt every `run()`. Immutables are scalars + perm-strings — read-only, never GC roots.
    pub(crate) immutables_cache: Vec<OnceLock<Vec<JsValue>>>,
    /// Read-only view into the currently-active module's converted immutables (inside immutables_cache).
    /// A fat `*const` because the cache Vec is VM-owned and run-stable (OnceLock filled once).
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) kernel_core: Arc<KernelCore>,
    pub(crate) session: KernelSession,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
    pub math_rng_state: u64,
    pub(crate) sub_modules: Arc<Vec<CompiledModule>>,
    pub(crate) saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    /// Shared register save-stack. Each active `CallFrame` saved its caller's live
    /// registers (`regs[..caller_reg_limit]`) here at `saved_reg_offset`; restore copies
    /// them back and truncates. Capacity is retained across calls — zero per-call heap alloc.
    pub(crate) save_stack: Vec<JsValue>,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) exception_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    pub(crate) root_reg_limit: u8,
    pub(crate) active_reg_limit: u8,
    pub(crate) native_call_depth: usize,
    /// Set to Some(target_reg) when `ordinary_get` pushes a bytecode accessor frame.
    /// The dispatch loop checks this flag and skips writing `regs[target_reg]` from the
    /// call result — the value will be delivered by the RETURN handler instead.
    pub(crate) accessor_frame_target_reg: Option<u8>,
    /// Grouped session-arena / GC bookkeeping.
    pub(crate) gc_state: GcState,
    /// Grouped `Symbol` interning state.
    pub(crate) symbols: SymbolState,
    /// Grouped live `for-in` / `for-of` iterator state.
    pub(crate) iters: IterState,
    /// Grouped inline-cache and instruction counters.
    pub(crate) profiling: ProfilingState,
}

impl Vm {
    const SYNC_NATIVE_ARG_BASE: usize = 0;
    const SYNC_NATIVE_ARG_LIMIT: usize = 253;

    pub(crate) fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        if !value.is_object() {
            return Ok(value);
        }

        let obj_ptr = value.as_js_object_ptr();
        if obj_ptr.is_null() {
            return Ok(value);
        }

        let method_names = if prefer_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };

        for method_name in method_names {
            let method_si = self.kernel_core.perm_interner().intern(method_name).0;
            let method = {
                let obj = unsafe { &*obj_ptr };
                self.ordinary_get(obj, method_si, value)?
            };
            if method.is_undefined() || method.is_null() {
                continue;
            }
            if !method.is_object() {
                self.raise_error_kind("TypeError", &format!("{method_name} is not callable"))?;
                return Ok(JsValue::undefined());
            }
            let method_ptr = method.as_js_object_ptr();
            if method_ptr.is_null() || !unsafe { &*method_ptr }.is_function() {
                self.raise_error_kind("TypeError", &format!("{method_name} is not callable"))?;
                return Ok(JsValue::undefined());
            }

            let result = self.call_function_sync(method, value, &[])?;
            if !result.is_object() {
                return Ok(result);
            }
        }

        Err(self.error_message_text("TypeError", "Cannot convert object to primitive value"))
    }

    pub(crate) fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        let primitive = self.coerce_primitive_bounded(value, false)?;
        Ok(coercion::to_number(primitive))
    }

    pub(crate) fn coerce_int32_bounded(&mut self, value: JsValue) -> Result<i32, String> {
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        let int = n.trunc().rem_euclid(4_294_967_296.0) as u32;
        if int > i32::MAX as u32 {
            Ok((int as i64 - 4_294_967_296i64) as i32)
        } else {
            Ok(int as i32)
        }
    }

    pub(crate) fn coerce_uint32_bounded(&mut self, value: JsValue) -> Result<u32, String> {
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        Ok(n.trunc().rem_euclid(4_294_967_296.0) as u32)
    }

    fn pack_sync_native_call_args(&mut self, receiver: JsValue, callee: JsValue, args: &[JsValue]) -> Vec<u8> {
        self.regs[253] = receiver;
        self.regs[254] = callee;

        let mut arg_regs = Vec::with_capacity(args.len() + 1);
        arg_regs.push(253);
        for (idx, arg) in args.iter().enumerate() {
            let reg = (Self::SYNC_NATIVE_ARG_BASE + idx) as u8;
            self.regs[reg as usize] = *arg;
            arg_regs.push(reg);
        }
        arg_regs
    }

    pub(crate) fn is_session_ptr(&self, obj_ptr: *mut JsObject) -> bool {
        if obj_ptr.is_null() {
            return false;
        }
        // SAFETY: obj_ptr is non-null and points to a `JsObject` owned by this session.
        unsafe { (*obj_ptr).is_session_epoch() }
    }

    pub(crate) fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        let ptr = self.epoch.alloc(obj);
        self.gc_state.track_epoch_object(ptr);
        ptr
    }

    /// Read-only view into the active module's converted immutables. Empty before any `run()`.
    #[inline(always)]
    pub(crate) fn immutables(&self) -> &[JsValue] {
        if self.active_immutables.is_null() {
            &[]
        } else {
            // SAFETY: active_immutables points into a OnceLock<Vec<JsValue>> inside immutables_cache,
            // which the VM owns; the Vec is filled once and never reallocated for the run's lifetime.
            unsafe { &*self.active_immutables }
        }
    }

    /// Activate module `cache_idx`'s immutables (0 = top, sub_idx+1 = sub_modules[sub_idx]),
    /// converting them once into `immutables_cache[cache_idx]` and pointing `active_immutables` at
    /// the cached Vec. `constants` is passed by the caller (it already holds `&module.constants`).
    pub(crate) fn activate_immutables(&mut self, cache_idx: usize, constants: &[Constant]) {
        // Raw-ptr the cache slot so `get_or_init` (which borrows immutables_cache) and the &self
        // convert_immutables closure don't conflict with the subsequent self.active_immutables write.
        // Sound: immutables_cache is VM-owned, read-only, and run-stable.
        let slot: *const OnceLock<Vec<JsValue>> = &self.immutables_cache[cache_idx];
        let vec = unsafe { &*slot }.get_or_init(|| self.convert_immutables(constants));
        self.active_immutables = vec.as_slice() as *const [JsValue];
    }

    pub(crate) fn for_each_root(&self, mut f: impl FnMut(JsValue)) {
        for value in &self.regs {
            if value.is_object() || value.is_string() {
                f(*value);
            }
        }
        for frame in &self.frames {
            f(frame.saved_this);
            f(frame.saved_new_target);
            f(frame.callee);
            f(frame.constructed_this.unwrap_or(JsValue::undefined()));
        }
        for &v in &self.save_stack {
            f(v);
        }
        f(JsValue::from_js_object(self.session.global_object().as_ptr() as *mut JsObject));
        f(self.exception_value.unwrap_or(JsValue::undefined()));
        f(self.pending_exception.unwrap_or(JsValue::undefined()));
        for &v in &self.iters.for_of_iters {
            f(v);
        }
        f(self.iters.last_for_of_result);
        // Converted immutables (scalars + perm-strings) are NOT session GC roots — not scanned.
        for iter in &self.iters.for_in_iters {
            if iter.is_null() {
                continue;
            }
            unsafe {
                for &v in (*(*iter)).keys.iter() {
                    f(v);
                }
            }
        }
    }

    pub(crate) fn maybe_collect_session_gc(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect(self);
        self.gc_state.session_gc = session_gc;
    }

    pub fn session_gc_stats(&self) -> &SessionGc {
        &self.gc_state.session_gc
    }

    pub fn session_object_count(&self) -> usize {
        self.gc_state.session_object_ptrs.len()
    }

    pub fn session_bytes_allocated(&self) -> usize {
        self.gc_state.session_bytes_allocated
    }

    pub fn epoch_object_count(&self) -> usize {
        self.gc_state.epoch_object_ptrs.len()
    }

    pub fn ic_hit_rate(&self) -> f64 {
        self.profiling.ic_hit_rate()
    }

    pub fn instruction_count(&self) -> u64 {
        self.profiling.instruction_count
    }

    pub fn symbol_registry_len(&self) -> usize {
        self.symbols.registry_len()
    }

    pub fn ic_hit_count(&self) -> u64 {
        self.profiling.ic_hits.get()
    }

    pub fn ic_miss_count(&self) -> u64 {
        self.profiling.ic_misses.get()
    }

    pub(crate) fn checked_object_ptr(
        &mut self, val: JsValue, error_msg: &str,
    ) -> Result<Option<*mut JsObject>, String> {
        if !val.is_object() {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        let ptr = val.as_js_object_ptr();
        let addr = ptr as usize;
        if ptr.is_null() || addr < 0x10000 || addr % std::mem::align_of::<JsObject>() != 0 {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        Ok(Some(ptr))
    }

    pub(crate) fn raise_error_kind(&mut self, kind: &'static str, msg: &str) -> Result<(), String> {
        self.raise_js_error(JsError::new(js_error_kind(kind), msg))
    }

    pub(crate) fn raise_js_error(&mut self, err: JsError) -> Result<(), String> {
        let kind = js_error_kind_name(err.kind);
        vm_debug!("raise_js_error: {} \"{}\"", kind, err.message);
        let error = oxide_builtins::error::create_kind_error(self, kind, &err.message);
        self.exception_value = Some(error);
        self.pending_error_kind = Some(kind);
        self.unwind()
    }

    pub(crate) fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_error_kind("TypeError", msg)
    }

    pub(crate) fn error_message_text(&self, kind: &str, msg: &str) -> String {
        format_error_message(kind, msg)
    }

    pub fn step_rng(&mut self) {
        if self.math_rng_state == 0 {
            self.math_rng_state = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            vm_debug!("step_rng: seeded");
        }
        self.math_rng_state = self
            .math_rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }

    pub fn math_rng_value(&self) -> f64 {
        (self.math_rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    pub fn kernel_core(&self) -> &Arc<KernelCore> {
        &self.kernel_core
    }

    pub fn session(&self) -> &KernelSession {
        &self.session
    }

    pub(crate) fn is_object_prototype(&self, ptr: *const JsObject) -> bool {
        let proto_ptr = self.session.builtin_world().object_proto.as_ptr();
        std::ptr::eq(ptr, proto_ptr)
    }

    pub fn reg(&self, idx: u8) -> JsValue {
        self.regs[idx as usize]
    }

    pub fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.regs[idx as usize] = val;
    }

    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        // SAFETY: val is a string value; its JsString pointer is alive for its lifetime.
        Some(unsafe { (*val.as_string_ptr()).data.clone() })
    }

    pub(crate) fn thrown_error_kind(&self, val: JsValue) -> &'static str {
        if !val.is_object() {
            return "Error";
        }
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        let obj = unsafe { &*val.as_js_object_ptr() };
        let Some(name_val) = self.resolve_property(obj, name_si) else {
            return "Error";
        };
        let Some(name) = self.lookup_str(name_val) else {
            return "Error";
        };
        match name.as_str() {
            "TypeError" => "TypeError",
            "ReferenceError" => "ReferenceError",
            "RangeError" => "RangeError",
            "SyntaxError" => "SyntaxError",
            "URIError" => "URIError",
            "EvalError" => "EvalError",
            "Error" => "Error",
            _ => "Error",
        }
    }

    pub(crate) fn property_key_si(&mut self, val: JsValue) -> u32 {
        if val.is_string() {
            // SAFETY: val is a string value; bridge its content to a permanent key id.
            let s = unsafe { &(*val.as_string_ptr()).data };
            return self.kernel_core.perm_interner().intern(s).0;
        }
        let key = coercion::to_string(val);
        self.kernel_core.perm_interner().intern(&key).0
    }

    pub(crate) fn array_index_from_property_key(&self, prop_name_si: u32) -> Option<u32> {
        let key = self.kernel_core.perm_interner().lookup(prop_name_si)?;
        if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
            return None;
        }
        key.parse::<u32>().ok()
    }

    pub(crate) fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        vm_trace!("resolve_property: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return Some(JsValue::int(obj.prop_count() as i32));
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                if index < obj.prop_vec_len() as u32 {
                    return Some(obj.get_prop_at(index));
                }
            }
        }
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self
                .kernel_core
                .shape_forge()
                .lookup_position(proto_obj.shape_id(), prop_name_si)
            {
                let val = proto_obj.get_prop_at(pos);
                if !val.is_undefined() || proto_obj.prop_vec_len() > pos as usize {
                    return Some(val);
                }
            }
            proto = proto_obj.proto();
        }
        None
    }

    pub(crate) fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return None;
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                if index < obj.prop_vec_len() as u32 {
                    return Some(index);
                }
            }
        }
        self.kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
            .and_then(|pos| {
                let val = obj.get_prop_at(pos);
                if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                    Some(pos)
                } else {
                    None
                }
            })
    }

    pub(crate) fn call_function_sync(
        &mut self, callee: JsValue, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        vm_debug!("call_function_sync: args={} callee_is_object={}", args.len(), callee.is_object());
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        if !callee_obj.is_function() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }

        if let Some(native_fn) = callee_obj.native_fn() {
            if self.native_call_depth >= self.kernel_core.config.max_call_depth {
                self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
                return Ok(JsValue::undefined());
            }
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
                return Ok(JsValue::undefined());
            }
            let saved_regs = self.regs;
            let arg_regs = self.pack_sync_native_call_args(receiver, callee, args);
            // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
            // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
            self.native_call_depth += 1;
            let result = func(self, &arg_regs);
            self.native_call_depth -= 1;
            self.regs = saved_regs;
            return match result {
                NativeResult::Ok(val) => Ok(val),
                NativeResult::Err(err) => Err(self.error_text(err)),
                NativeResult::TailCall { callee, this, args } => self.call_function_sync(callee, this, &args),
            };
        }

        // Run bytecode function inline on self (same epoch) to prevent use-after-free.
        // A separate sub-VM would own a different epoch; returning a JsValue that contains
        // a pointer into the sub-VM epoch and then dropping the sub-VM causes a dangling
        // pointer / access violation in release builds.
        self.call_bytecode_function_inline(callee, callee_obj, receiver, args)
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn push_bytecode_frame(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue], construct_result_reg: Option<u8>,
        constructed_this: Option<JsValue>, new_target: JsValue, continuation: FrameContinuation,
    ) -> Result<(), String> {
        vm_trace!(
            "push_bytecode_frame: depth={}, args={}, continuation={:?}",
            self.frames.len(),
            args.len(),
            continuation
        );
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        if !obj.is_function() || obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let sub_idx = obj.sub_module_index() as usize - 1;
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "CALL: sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel_core.config.max_call_depth {
            return self.raise_error_kind("RangeError", "Maximum call stack size exceeded");
        }

        let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
        let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
        let sub_n_registers = self.sub_modules[sub_idx].n_registers;
        let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
        let sub_is_arrow = self.sub_modules[sub_idx].is_arrow;
        let caller_reg_limit = self.active_reg_limit.max(1);
        let saved_reg_offset = self.save_stack.len() as u32;
        self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
        let saved_this = self.regs[254];
        let saved_new_target = self.regs[255];

        for i in 0..sub_n_args {
            self.regs[sub_param_base + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub_is_arrow { obj.captured_this() } else { this_value };
        self.regs[255] = new_target;

        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
        self.saved_immutables_stack.push(self.active_immutables);

        let function_name = self.sub_modules[sub_idx]
            .function_name
            .as_deref()
            .map(|name| self.kernel_core.perm_interner().intern(name).0)
            .unwrap_or(0);

        self.frames.push(CallFrame {
            return_addr: self.pc,
            function_name,
            caller_reg_limit,
            saved_reg_offset,
            saved_this,
            saved_new_target,
            callee,
            construct_result_reg,
            constructed_this,
            is_derived_constructor: obj.is_derived_constructor(),
            continuation,
        });

        self.bytecode = sub_bytecode;
        let subs = Arc::clone(&self.sub_modules);
        self.activate_immutables(sub_idx + 1, &subs[sub_idx].constants);
        for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map.clone() {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.active_reg_limit = sub_n_registers.max(1);
        self.pc = 0;
        Ok(())
    }

    pub(crate) fn dispatch(&mut self) -> Result<JsValue, String> {
        let mut steps: u64 = 0;
        loop {
            steps += 1;
            if let Some(max_steps) = self.kernel_core.config.max_steps {
                if steps > max_steps {
                    vm_warn!("dispatch: step limit {} exceeded at pc={}", max_steps, self.pc);
                    self.profiling.set_instruction_count(steps);
                    return Err(format!("VM step limit exceeded at pc={}", self.pc));
                }
            }
            if self.pc >= self.bytecode.len() {
                vm_error!("dispatch: program counter out of bounds pc={} len={}", self.pc, self.bytecode.len());
                self.profiling.instruction_count = steps;
                return Err("program counter out of bounds".into());
            }

            let instr = self.bytecode[self.pc];
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr) as usize;
            let a = opcode::a(instr) as usize;
            let b = opcode::b(instr) as usize;
            self.pc += 1;

            match op {
                OpCode::NOP => {}

                OpCode::HALT => {
                    vm_trace!("HALT: regs[0]={:?}", self.regs[0]);
                    self.profiling.set_instruction_count(steps);
                    return Ok(self.regs[0]);
                }

                OpCode::LOAD_CONST => {
                    self.dispatch_load_const(rd, instr)?;
                }

                OpCode::CREATE_CLOSURE => {
                    self.dispatch_create_closure(rd, instr);
                }
                OpCode::CREATE_REGEXP => match self.dispatch_create_regexp(rd, a, b) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::ADD => {
                    self.dispatch_add(rd, a, b)?;
                }

                OpCode::SUB => {
                    binary_arith!(self, a, b, rd, -);
                }

                OpCode::MUL => {
                    binary_arith!(self, a, b, rd, *);
                }

                OpCode::DIV => {
                    binary_arith!(self, a, b, rd, /);
                }

                OpCode::MOD => {
                    binary_arith!(self, a, b, rd, %);
                }

                OpCode::NEG => {
                    self.dispatch_neg(rd, a)?;
                }

                OpCode::BIT_AND => {
                    self.dispatch_bit_and(rd, a, b)?;
                }

                OpCode::BIT_OR => {
                    self.dispatch_bit_or(rd, a, b)?;
                }

                OpCode::BIT_XOR => {
                    self.dispatch_bit_xor(rd, a, b)?;
                }

                OpCode::SHL => {
                    self.dispatch_shl(rd, a, b)?;
                }

                OpCode::SHR => {
                    self.dispatch_shr(rd, a, b)?;
                }

                OpCode::USHR => {
                    self.dispatch_ushr(rd, a, b)?;
                }

                OpCode::BIT_NOT => {
                    self.dispatch_bit_not(rd, a)?;
                }

                OpCode::EQ => {
                    self.dispatch_eq(rd, a, b);
                }

                OpCode::NEQ => {
                    self.dispatch_neq(rd, a, b);
                }

                OpCode::LT => {
                    self.dispatch_lt(rd, a, b);
                }

                OpCode::GT => {
                    self.dispatch_gt(rd, a, b);
                }

                OpCode::LTE => {
                    self.dispatch_lte(rd, a, b);
                }

                OpCode::GTE => {
                    self.dispatch_gte(rd, a, b);
                }

                OpCode::STRICT_EQ => {
                    self.dispatch_strict_eq(rd, a, b);
                }

                OpCode::STRICT_NEQ => {
                    self.dispatch_strict_neq(rd, a, b);
                }

                OpCode::UNARY_PLUS => {
                    self.dispatch_unary_plus(rd, a)?;
                }

                OpCode::JMP => {
                    self.dispatch_jmp(instr);
                }

                OpCode::JMP_IF_FALSE => {
                    self.dispatch_jmp_if_false(rd, instr);
                }

                OpCode::JMP_IF_TRUE => {
                    self.dispatch_jmp_if_true(rd, instr);
                }

                OpCode::JMP_IF_NULLISH => {
                    self.dispatch_jmp_if_nullish(rd, instr);
                }

                OpCode::LOAD_VAR => match self.dispatch_load_var(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::STORE_VAR => match self.dispatch_store_var(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::CALL => match self.dispatch_call(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::CALL_NATIVE => {
                    self.dispatch_call_native(rd, a, b)?;
                }

                OpCode::NEW_EXPRESSION => match self.dispatch_new_expression(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_CALL => match self.dispatch_super_call(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_GET_PROP | OpCode::SUPER_STATIC_GET_PROP => {
                    match self.dispatch_super_get_prop(rd, a, b) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => return Err(e),
                    }
                }

                OpCode::SET_HOME_OBJECT => match self.dispatch_set_home_object(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DEFINE_ACCESSOR => {
                    self.dispatch_define_accessor(rd, a, b)?;
                }

                OpCode::RETURN => match self.dispatch_return(rd) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::GET_PROP
                | OpCode::SET_PROP
                | OpCode::GET_PROP_DYNAMIC
                | OpCode::SET_PROP_DYNAMIC
                | OpCode::SET_ELEM
                | OpCode::GET_PRIVATE
                | OpCode::SET_PRIVATE
                | OpCode::INIT_PRIVATE
                | OpCode::PRIVATE_BRAND_IN => {
                    self.dispatch_property_op(op, rd, a, b)?;
                }

                OpCode::NEW_OBJECT => {
                    self.dispatch_new_object(rd);
                }

                OpCode::NEW_ARRAY => {
                    self.dispatch_new_array(rd, instr);
                }

                OpCode::COMPOUND_ADD => {
                    self.dispatch_compound_add(rd, a)?;
                }

                OpCode::COMPOUND_SUB => {
                    self.dispatch_compound_sub(rd, a)?;
                }

                OpCode::COMPOUND_MUL => {
                    self.dispatch_compound_mul(rd, a)?;
                }

                OpCode::COMPOUND_DIV => {
                    self.dispatch_compound_div(rd, a)?;
                }

                OpCode::COMPOUND_MOD => {
                    self.dispatch_compound_mod(rd, a)?;
                }

                OpCode::COMPOUND_EXP => {
                    self.dispatch_compound_exp(rd, a)?;
                }

                OpCode::COMPOUND_AND => {
                    self.dispatch_compound_bit_and(rd, a)?;
                }

                OpCode::COMPOUND_OR => {
                    self.dispatch_compound_bit_or(rd, a)?;
                }

                OpCode::COMPOUND_XOR => {
                    self.dispatch_compound_bit_xor(rd, a)?;
                }

                OpCode::COMPOUND_SHL => {
                    self.dispatch_compound_shl(rd, a)?;
                }

                OpCode::COMPOUND_SHR => {
                    self.dispatch_compound_shr(rd, a)?;
                }

                OpCode::COMPOUND_USHR => {
                    self.dispatch_compound_ushr(rd, a)?;
                }

                OpCode::TYPEOF => {
                    self.dispatch_typeof(rd, a);
                }

                OpCode::VOID => {
                    self.dispatch_void(rd);
                }

                OpCode::TEMPLATE_STR => {
                    self.dispatch_template_str(rd);
                }

                OpCode::DELETE_PROP_STATIC => match self.dispatch_delete_prop_static(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DELETE_PROP_DYNAMIC => match self.dispatch_delete_prop_dynamic(rd, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::INSTANCEOF => {
                    self.dispatch_instanceof(rd, a, b)?;
                }

                OpCode::IN => {
                    self.dispatch_in(rd, a, b)?;
                }

                OpCode::NOT => {
                    self.dispatch_not(rd, a);
                }

                OpCode::AND => {
                    self.dispatch_and(rd, a, b);
                }

                OpCode::OR => {
                    self.dispatch_or(rd, a, b);
                }

                OpCode::NULLISH => {
                    self.dispatch_nullish(rd, a, b);
                }

                OpCode::INC_PRE => {
                    self.dispatch_inc_pre(rd, a)?;
                }

                OpCode::INC_POST => {
                    self.dispatch_inc_post(rd, a)?;
                }

                OpCode::DEC_PRE => {
                    self.dispatch_dec_pre(rd, a)?;
                }

                OpCode::DEC_POST => {
                    self.dispatch_dec_post(rd, a)?;
                }

                OpCode::MEMBER_INC
                | OpCode::MEMBER_DEC
                | OpCode::DYN_MEMBER_INC
                | OpCode::DYN_MEMBER_DEC
                | OpCode::COMPOUND_MEMBER_ADD
                | OpCode::COMPOUND_MEMBER_SUB
                | OpCode::COMPOUND_MEMBER_MUL
                | OpCode::COMPOUND_MEMBER_DIV
                | OpCode::COMPOUND_MEMBER_MOD
                | OpCode::COMPOUND_MEMBER_EXP => {
                    self.dispatch_member_op(op, rd, a, b)?;
                }

                OpCode::FOR_IN_INIT => {
                    self.dispatch_for_in_init(a)?;
                }

                OpCode::FOR_IN_NEXT => {
                    self.dispatch_for_in_next(rd)?;
                }

                OpCode::FOR_IN_DONE => {
                    self.dispatch_for_in_done(rd);
                }

                OpCode::FOR_IN_CLEANUP => {
                    self.dispatch_for_in_cleanup();
                }

                OpCode::FOR_OF_INIT => {
                    self.dispatch_for_of_init(a)?;
                }

                OpCode::FOR_OF_NEXT => {
                    self.dispatch_for_of_next(rd)?;
                }

                OpCode::FOR_OF_DONE => {
                    self.dispatch_for_of_done(rd)?;
                }

                OpCode::FOR_OF_CLOSE => {
                    self.dispatch_for_of_close()?;
                }

                OpCode::REST_OBJECT => {
                    self.dispatch_rest_object(rd, a)?;
                }

                OpCode::THROW => match self.dispatch_throw(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::TRY_BEGIN => {
                    self.dispatch_try_begin(instr);
                }

                OpCode::TRY_END => {
                    self.dispatch_try_end();
                }

                OpCode::TRY_FINALLY_BEGIN => {
                    self.dispatch_try_finally_begin(instr);
                }

                OpCode::TRY_FINALLY_END => match self.dispatch_try_finally_end() {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                _ => {
                    return Err(format!("opcode {op} not yet implemented"));
                }
            }
        }
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{opcode, JsValue, TryHandler, Vm};
    use oxide_bytecode::module::CompiledModule;
    use oxide_runtime_api::NativeResult;
    use oxide_types::object::NativeFnPtr;
    use oxide_types::object::{JsObject, PropAttributes};

    fn native_return_7(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(7))
    }

    fn native_get_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        NativeResult::Ok(vm.resolve_property(obj, marker_si).unwrap_or(JsValue::undefined()))
    }

    fn native_set_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        let value = vm.reg(args[1]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, marker_si, value);
        NativeResult::Ok(JsValue::undefined())
    }

    fn native_return_last_arg(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let reg = *args.last().expect("receiver + args");
        NativeResult::Ok(vm.reg(reg))
    }

    fn native_return_arg_count(_vm: &mut Vm, args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(args.len().saturating_sub(1) as i32))
    }

    fn native_function(vm: &mut Vm, f: crate::native::NativeFn) -> JsValue {
        let proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let mut obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: f is a NativeFn fn-item; valid to store as NativeFnPtr.
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(f as *const ()) }));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn plain_object(vm: &mut Vm) -> JsValue {
        let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn add_accessor(vm: &mut Vm, obj_val: JsValue, name: &str, get: JsValue, set: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let shape_id = vm.kernel_core.shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape_id);
        let pos = obj.push_prop(JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, PropAttributes::DEFAULT_DATA);
        obj.bump_generation();
    }

    fn set_data(vm: &mut Vm, obj_val: JsValue, name: &str, val: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, si, val);
    }

    #[test]
    fn reset_clears_runtime_state_like_rerun() {
        let mut vm = Vm::new();
        vm.regs[1] = JsValue::int(7);
        vm.pc = 3;
        vm.frames.push(super::CallFrame {
            return_addr: 1,
            function_name: 0,
            caller_reg_limit: 2,
            saved_reg_offset: 0,
            saved_this: JsValue::undefined(),
            saved_new_target: JsValue::undefined(),
            callee: JsValue::undefined(),
            construct_result_reg: None,
            constructed_this: None,
            is_derived_constructor: false,
            continuation: super::FrameContinuation::None,
        });
        vm.save_stack.push(JsValue::undefined());
        vm.iters
            .for_in_iters
            .push(std::ptr::dangling_mut::<super::ForInIter<'static>>());
        vm.iters.for_of_iters.push(JsValue::undefined());
        vm.saved_bytecode_stack
            .push(vec![opcode::encode(opcode::OpCode::HALT, 0, 0, 0)]);
        vm.saved_immutables_stack
            .push(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
        vm.try_stack.push(TryHandler {
            catch_pc: Some(1),
            finally_pc: None,
            frame_depth: 0,
        });
        vm.exception_value = Some(JsValue::int(2));
        vm.pending_exception = Some(JsValue::int(3));
        vm.pending_error_kind = Some("TypeError");

        vm.reset();

        assert_eq!(vm.pc, 0);
        assert!(vm.frames.is_empty());
        assert!(vm.save_stack.is_empty());
        assert!(vm.iters.for_in_iters.is_empty());
        assert!(vm.iters.for_of_iters.is_empty());
        assert!(vm.saved_bytecode_stack.is_empty());
        assert!(vm.saved_immutables_stack.is_empty());
        assert!(vm.try_stack.is_empty());
        assert!(vm.exception_value.is_none());
        assert!(vm.pending_exception.is_none());
        assert!(vm.pending_error_kind.is_none());
        assert!(vm.bytecode.is_empty());
        assert!(vm.immutables().is_empty());
    }

    #[test]
    fn full_reset_clears_symbol_state() {
        let mut vm = Vm::new();
        vm.symbols.intern("shared".to_string());

        vm.full_reset();

        assert_eq!(vm.symbols.symbol_counter, 0);
        assert!(vm.symbols.symbol_descriptions.is_empty());
        assert!(vm.symbols.symbol_registry.is_empty());
    }

    #[test]
    fn for_of_close_pops_iterator_stack() {
        let module = CompiledModule {
            bytecode: vec![
                opcode::encode(opcode::OpCode::FOR_OF_CLOSE, 0, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ],
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        vm.iters.for_of_iters.push(JsValue::undefined());

        vm.run(&module).expect("FOR_OF_CLOSE should tolerate non-object sentinel");

        assert!(vm.iters.for_of_iters.is_empty());
    }

    #[test]
    fn write_ic_back_updates_three_extension_words() {
        let mut vm = Vm::new();
        vm.bytecode = vec![0, 0, 0];
        vm.pc = 3;
        crate::ic_helper::write_ic_back(&mut vm.bytecode, vm.pc, 0x1234_5678, 7);
        assert_eq!(vm.bytecode[0], 0x0034_5678);
        assert_eq!(vm.bytecode[1], 7);
        assert_eq!(vm.bytecode[2], 0);
    }

    #[test]
    fn unimplemented_profile_opcode_fails_explicitly() {
        let module = CompiledModule {
            bytecode: vec![
                opcode::encode(opcode::OpCode::PROFILE_TYPE, 0, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ],
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        let err = vm.run(&module).expect_err("unimplemented opcode should fail explicitly");
        assert_eq!(err, "opcode PROFILE_TYPE not yet implemented");
    }

    #[test]
    fn ordinary_get_calls_own_native_getter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_return_7);
        add_accessor(&mut vm, obj_val, "x", getter, JsValue::undefined());

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        let value = vm.ordinary_get(obj, x_si, obj_val).expect("getter");
        assert_eq!(value, JsValue::int(7));
    }

    #[test]
    fn ordinary_set_calls_own_native_setter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, obj_val, "x", JsValue::undefined(), setter);

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.ordinary_set(obj, x_si, JsValue::int(9), obj_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(obj, marker_si), Some(JsValue::int(9)));
    }

    #[test]
    fn inherited_getter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_get_marker);
        add_accessor(&mut vm, proto_val, "x", getter, JsValue::undefined());
        set_data(&mut vm, child_val, "marker", JsValue::int(42));
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let value = vm.ordinary_get(child, x_si, child_val).expect("getter");
        assert_eq!(value, JsValue::int(42));
    }

    #[test]
    fn inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(12), child_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let proto = unsafe { &*proto_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(12)));
        assert_eq!(vm.resolve_property(proto, marker_si), None);
    }

    #[test]
    fn deep_inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let grand_proto_val = plain_object(&mut vm);
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, grand_proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*proto_val.as_js_object_ptr()).set_proto(grand_proto_val).expect("proto");
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(15), child_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(15)));
    }

    #[test]
    fn ordinary_data_property_still_reads_and_writes_without_meta() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        set_data(&mut vm, obj_val, "x", JsValue::int(1));

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        assert!(!obj.has_prop_meta());
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(1));
        vm.ordinary_set(obj, x_si, JsValue::int(2), obj_val).expect("set");
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(2));
    }

    #[test]
    fn call_function_sync_passes_high_arity_native_args_without_truncation() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_last_arg);
        let args: Vec<JsValue> = (0..20).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("high-arity sync call should succeed");

        assert_eq!(result, JsValue::int(19));
    }

    #[test]
    fn call_function_sync_reports_actual_native_arg_count() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_arg_count);
        let args: Vec<JsValue> = (0..32).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("arg count should be preserved");

        assert_eq!(result, JsValue::int(32));
    }

    #[test]
    fn call_function_sync_rejects_unrepresentable_native_arity_without_register_corruption() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_arg_count);
        vm.set_reg(1, JsValue::int(7));
        vm.set_reg(253, JsValue::int(11));
        vm.set_reg(254, JsValue::int(12));
        vm.set_reg(255, JsValue::int(13));

        let args = vec![JsValue::undefined(); Vm::SYNC_NATIVE_ARG_LIMIT + 1];
        let err = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect_err("too many args should fail cleanly");

        assert!(err.contains("Maximum call stack size exceeded"), "unexpected error: {err}");
        assert_eq!(vm.reg(1), JsValue::int(7));
        assert_eq!(vm.reg(253), JsValue::int(11));
        assert_eq!(vm.reg(254), JsValue::int(12));
        assert_eq!(vm.reg(255), JsValue::int(13));
    }
}

impl oxide_runtime_api::VmHost for Vm {
    fn reg(&self, idx: u8) -> JsValue {
        self.reg(idx)
    }
    fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.set_reg(idx, val);
    }
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        self.alloc_object(obj)
    }
    fn new_string(&mut self, s: &str) -> JsValue {
        self.new_string(s)
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
    fn property_key_si(&mut self, val: JsValue) -> u32 {
        self.property_key_si(val)
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
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set(obj, prop_name_si, val, receiver)
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
    fn sub_module_function_name(&self, sub_idx: u16) -> String {
        self.sub_modules
            .get(sub_idx as usize)
            .and_then(|m| m.function_name.clone())
            .unwrap_or_default()
    }
    fn symbol_intern(&mut self, desc: String) -> u32 {
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
