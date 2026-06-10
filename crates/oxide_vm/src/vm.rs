#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::bindings;
pub use crate::bindings::init_kernel_builtins;
use crate::coercion;
use crate::native::{NativeFn, NativeResult};
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

/// Convert a `NativeFnPtr` to a callable `NativeFn`.
///
/// # Safety
/// The pointer stored in `ptr` must have been created from a valid `NativeFn` fn-item.
/// This is the single point in the codebase where `NativeFnPtr → NativeFn` coercion happens.
#[inline(always)]
pub(crate) unsafe fn native_fn_ptr_to_fn(ptr: NativeFnPtr) -> NativeFn {
    std::mem::transmute::<*const (), NativeFn>(ptr.as_ptr())
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
        let l = coercion::to_number($self.regs[$a], $self.kernel.string_forge().as_ref());
        let r = coercion::to_number($self.regs[$b], $self.kernel.string_forge().as_ref());
        $self.regs[$rd] = JsValue::float(l $op r);
    }}
}

macro_rules! compound_arith {
    ($self:ident, $rd:expr, $a:expr, $op:tt) => {{
        let l = coercion::to_number($self.regs[$rd], $self.kernel.string_forge().as_ref());
        let r = coercion::to_number($self.regs[$a], $self.kernel.string_forge().as_ref());
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
    pub saved_regs: Box<[JsValue]>,
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

pub struct Vm {
    pub(crate) regs: [JsValue; 256],
    pub(crate) pc: usize,
    pub(crate) bytecode: Vec<opcode::Instr>,
    pub(crate) constants: Vec<JsValue>,
    pub(crate) frames: Vec<CallFrame>,
    pub for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) kernel: Arc<OxideKernel>,
    pub(crate) interned_strings: Vec<u32>,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
    pub math_rng_state: u64,
    pub(crate) sub_modules: Vec<CompiledModule>,
    pub(crate) sub_module_constants: Vec<Vec<JsValue>>,
    pub(crate) saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    pub(crate) saved_constants_stack: Vec<Vec<JsValue>>,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) exception_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    pub(crate) symbol_counter: u32,
    pub(crate) symbol_descriptions: Vec<String>,
    #[allow(dead_code)]
    pub(crate) for_of_iters: Vec<*mut u8>,
    pub(crate) root_reg_limit: u8,
    pub(crate) active_reg_limit: u8,
    pub(crate) native_call_depth: usize,
    /// Set to Some(target_reg) when `ordinary_get` pushes a bytecode accessor frame.
    /// The dispatch loop checks this flag and skips writing `regs[target_reg]` from the
    /// call result — the value will be delivered by the RETURN handler instead.
    pub(crate) accessor_frame_target_reg: Option<u8>,
}

impl Vm {
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
        let error = match kind {
            "TypeError" => crate::builtins::error::create_type_error(self, msg),
            "ReferenceError" => crate::builtins::error::create_reference_error(self, msg),
            "SyntaxError" => crate::builtins::error::create_syntax_error(self, msg),
            _ => crate::builtins::error::create_error(self, msg),
        };
        self.exception_value = Some(error);
        self.pending_error_kind = Some(kind);
        self.unwind()
    }

    pub(crate) fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_error_kind("TypeError", msg)
    }

    pub fn new() -> Self {
        let kernel = Arc::new(OxideKernel::new(KernelConfig::minimal()));
        bindings::init_kernel_builtins(&kernel);
        let obj_proto = P::clone(&kernel.builtin_world().object_proto);
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            for_in_iters: Vec::new(),
            kernel,
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            math_rng_state: 0,
            sub_modules: Vec::new(),
            sub_module_constants: Vec::new(),
            saved_bytecode_stack: Vec::new(),
            saved_constants_stack: Vec::new(),
            try_stack: Vec::new(),
            exception_value: None,
            pending_exception: None,
            pending_error_kind: None,
            symbol_counter: 0,
            symbol_descriptions: Vec::new(),
            for_of_iters: Vec::new(),
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
        }
    }

    pub fn with_kernel(kernel: Arc<OxideKernel>) -> Self {
        let obj_proto = P::clone(&kernel.builtin_world().object_proto);
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            for_in_iters: Vec::new(),
            kernel,
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            math_rng_state: 0,
            sub_modules: Vec::new(),
            sub_module_constants: Vec::new(),
            saved_bytecode_stack: Vec::new(),
            saved_constants_stack: Vec::new(),
            try_stack: Vec::new(),
            exception_value: None,
            pending_exception: None,
            pending_error_kind: None,
            symbol_counter: 0,
            symbol_descriptions: Vec::new(),
            for_of_iters: Vec::new(),
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
        }
    }

    pub fn step_rng(&mut self) {
        if self.math_rng_state == 0 {
            self.math_rng_state = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
        }
        self.math_rng_state = self
            .math_rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }

    pub fn math_rng_value(&self) -> f64 {
        (self.math_rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    pub fn kernel(&self) -> &Arc<OxideKernel> {
        &self.kernel
    }

    pub(crate) fn is_object_prototype(&self, ptr: *const JsObject) -> bool {
        let proto_ptr = self.kernel.builtin_world().object_proto.as_ptr();
        std::ptr::eq(ptr, proto_ptr)
    }

    pub fn reg(&self, idx: u8) -> JsValue {
        self.regs[idx as usize]
    }

    pub fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.regs[idx as usize] = val;
    }

    pub fn regs_mut(&mut self) -> &mut [JsValue; 256] {
        &mut self.regs
    }

    pub fn epoch_mut(&mut self) -> &mut Epoch {
        &mut self.epoch
    }

    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    fn clear_execution_state(&mut self) {
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.frames.clear();
        self.for_in_iters.clear();
        self.for_of_iters.clear();
        self.saved_bytecode_stack.clear();
        self.saved_constants_stack.clear();
        self.try_stack.clear();
        self.exception_value = None;
        self.pending_exception = None;
        self.pending_error_kind = None;
        self.native_call_depth = 0;
    }

    pub fn reset(&mut self) {
        self.clear_execution_state();
        self.bytecode.clear();
        self.constants.clear();
        self.epoch.reset();
        self.interned_strings.clear();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    pub fn intern(&mut self, s: &str) -> JsValue {
        let (idx, hash) = self.kernel.string_forge().intern(s);
        self.interned_strings.push(idx);
        JsValue::string(idx, hash)
    }

    /// Create a function JsObject for a BytecodeFunc constant.
    /// When `is_arrow` is true, captures the current `this` (regs[254])
    /// for lexical this binding at call time (D-01).
    fn create_function_object(
        &mut self, sub_idx: u32, is_arrow: bool, is_class_constructor: bool, is_derived_constructor: bool,
        needs_home_object: bool,
    ) -> JsValue {
        let func_proto_ptr = self.kernel.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(func_proto_ptr);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
        obj.set_function(true);
        obj.set_sub_module_index(sub_idx);
        obj.set_class_constructor(is_class_constructor);
        obj.set_derived_constructor(is_derived_constructor);
        let _ = needs_home_object;
        if is_arrow {
            obj.set_arrow(true);
            // Capture lexical `this` from the enclosing scope (regs[254]).
            obj.set_captured_this(self.regs[254]);
        }
        let obj_ptr = self.epoch.alloc(obj);
        let func_val = JsValue::object(obj_ptr as *mut u8);

        if !is_arrow {
            let object_proto_ptr = self.kernel.builtin_world().object_proto.as_ptr() as *mut JsObject;
            let prototype_obj = self
                .epoch
                .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto_ptr)));
            let prototype_val = JsValue::from_js_object(prototype_obj);

            let constructor_si = self.kernel.string_forge().intern("constructor").0;
            let constructor_shape = self.kernel.shape_forge().make_shape(EMPTY_SHAPE_ID, constructor_si);
            let prototype = unsafe { &mut *prototype_obj };
            prototype.set_shape_id(constructor_shape);
            let constructor_pos = prototype.push_prop(func_val);
            prototype.set_data_meta(constructor_pos, PropAttributes::new(true, false, true));
            prototype.bump_generation();

            let prototype_si = self.kernel.string_forge().intern("prototype").0;
            let func = unsafe { &mut *obj_ptr };
            let prototype_shape = self.kernel.shape_forge().make_shape(func.shape_id(), prototype_si);
            func.set_shape_id(prototype_shape);
            func.ensure_hash_props().push(Box::new(prototype_val));
            func.bump_generation();
        }

        func_val
    }

    fn error_text(&self, val: JsValue) -> String {
        if let Some(s) = self.lookup_str(val) {
            return s;
        }
        if val.is_object() {
            let obj = unsafe { &*val.as_js_object_ptr() };
            let name_si = self.kernel.string_forge().intern("name").0;
            let message_si = self.kernel.string_forge().intern("message").0;
            let name = self
                .resolve_property(obj, name_si)
                .and_then(|v| self.lookup_str(v))
                .unwrap_or_else(|| "Error".to_string());
            let message = self
                .resolve_property(obj, message_si)
                .and_then(|v| self.lookup_str(v))
                .unwrap_or_default();
            return if message.is_empty() { name } else { format!("{name}: {message}") };
        }
        format!("{val}")
    }

    fn convert_constant(&mut self, constant: &Constant) -> Result<JsValue, String> {
        match constant {
            Constant::Number(v) => Ok(JsValue::float(*v)),
            Constant::Int(v) => Ok(JsValue::int(*v)),
            Constant::String(s) => Ok(self.intern(s)),
            Constant::Boolean(b) => Ok(JsValue::bool(*b)),
            Constant::Null => Ok(JsValue::null()),
            Constant::Undefined => Ok(JsValue::undefined()),
            Constant::BytecodeFunc(idx) => {
                let sub_idx = *idx as usize;
                let (is_arrow, is_class_constructor, is_derived_constructor, needs_home_object) =
                    if sub_idx > 0 && sub_idx <= self.sub_modules.len() {
                        let sub_module = &self.sub_modules[sub_idx - 1];
                        (
                            sub_module.is_arrow,
                            sub_module.is_class_constructor,
                            sub_module.is_derived_constructor,
                            sub_module.needs_home_object,
                        )
                    } else {
                        (false, false, false, false)
                    };
                Ok(self.create_function_object(
                    *idx,
                    is_arrow,
                    is_class_constructor,
                    is_derived_constructor,
                    needs_home_object,
                ))
            }
            Constant::RegExp(pattern, flags) => {
                let pat_si = self.kernel.string_forge().intern(pattern).0;
                let flags_si = self.kernel.string_forge().intern(flags).0;
                let pat_val = JsValue::string(pat_si, 0);
                let flags_val = JsValue::string(flags_si, 0);

                let ctor_ptr = self.kernel.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
                let ctor = unsafe { &*ctor_ptr };
                let Some(native_fn) = ctor.native_fn() else {
                    return Err("SyntaxError: RegExp constructor unavailable".into());
                };

                let saved_0 = self.regs[0];
                let saved_1 = self.regs[1];
                let saved_2 = self.regs[2];
                self.regs[0] = JsValue::undefined();
                self.regs[1] = pat_val;
                self.regs[2] = flags_val;
                // SAFETY: regexp constructor was registered via set_native_fn with a valid NativeFn pointer;
                // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
                let func: crate::native::NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
                let result = func(self, &[0, 1, 2]);
                self.regs[0] = saved_0;
                self.regs[1] = saved_1;
                self.regs[2] = saved_2;
                result.map_err(|err| self.error_text(err))
            }
        }
    }

    /// Convert a module constant pool into runtime values.
    fn convert_constants(&mut self, constants: &[Constant]) -> Result<Vec<JsValue>, String> {
        let mut values = Vec::with_capacity(constants.len());
        for constant in constants {
            values.push(self.convert_constant(constant)?);
        }
        Ok(values)
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        self.kernel.string_forge().lookup(val.as_string_index())
    }

    pub(crate) fn thrown_error_kind(&self, val: JsValue) -> &'static str {
        if !val.is_object() {
            return "Error";
        }
        let name_si = self.kernel.string_forge().intern("name").0;
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
            return val.as_string_index();
        }
        let key = coercion::to_string(self.kernel.string_forge().as_ref(), val);
        self.kernel.string_forge().intern(&key).0
    }

    pub(crate) fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        let length_si = self.kernel.string_forge().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return Some(JsValue::int(obj.prop_count() as i32));
        }
        if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self.kernel.shape_forge().lookup_position(proto_obj.shape_id(), prop_name_si) {
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
        let length_si = self.kernel.string_forge().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return None;
        }
        self.kernel
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
        if !callee.is_object() {
            return Err("TypeError: accessor is not callable".into());
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        if !callee_obj.is_function() {
            return Err("TypeError: accessor is not callable".into());
        }

        if let Some(native_fn) = callee_obj.native_fn() {
            if self.native_call_depth >= self.kernel.config.max_call_depth {
                return Err("RangeError: Maximum call stack size exceeded".into());
            }
            let saved_regs = self.regs;
            let saved_this = self.regs[254];
            let saved_callee = self.regs[253];
            self.regs[253] = receiver;
            self.regs[254] = callee;
            for (idx, arg) in args.iter().enumerate() {
                self.regs[240 + idx] = *arg;
            }
            let mut arg_regs = [0u8; 17];
            arg_regs[0] = 253;
            for idx in 0..args.len().min(16) {
                arg_regs[idx + 1] = 240 + idx as u8;
            }
            // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
            // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
            self.native_call_depth += 1;
            let result = func(self, &arg_regs[..args.len().min(16) + 1]);
            self.native_call_depth -= 1;
            self.regs = saved_regs;
            self.regs[254] = saved_this;
            self.regs[253] = saved_callee;
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

    /// Execute a bytecode user function on `self`, saving and restoring all execution state.
    /// Objects allocated during the call live in `self.epoch` and remain valid after return.
    fn call_bytecode_function_inline(
        &mut self, callee: JsValue, callee_obj: &JsObject, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        if callee_obj.sub_module_index() == 0 {
            return Err("TypeError: accessor is not callable".into());
        }
        let sub_idx = callee_obj.sub_module_index() as usize - 1;
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "accessor sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel.config.max_call_depth {
            return Err("RangeError: Maximum call stack size exceeded".into());
        }

        let sub = self.sub_modules[sub_idx].clone();
        let converted_constants = self.convert_constants(&sub.constants)?;

        // Save execution state.
        let saved_regs = self.regs;
        let saved_pc = self.pc;
        let saved_bytecode = std::mem::take(&mut self.bytecode);
        let saved_constants = std::mem::take(&mut self.constants);
        let saved_active_reg_limit = self.active_reg_limit;
        let saved_root_reg_limit = self.root_reg_limit;
        let saved_try_stack = std::mem::take(&mut self.try_stack);
        let saved_frames = std::mem::take(&mut self.frames);
        let saved_exception = self.exception_value.take();
        let saved_pending = self.pending_exception.take();
        let saved_pending_kind = self.pending_error_kind.take();
        let saved_for_in_iters = std::mem::take(&mut self.for_in_iters);
        let saved_saved_bytecode_stack = std::mem::take(&mut self.saved_bytecode_stack);
        let saved_saved_constants_stack = std::mem::take(&mut self.saved_constants_stack);

        // Set up callee.
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode = sub.bytecode;
        self.constants = converted_constants;
        self.active_reg_limit = sub.n_registers.max(1);
        self.root_reg_limit = self.active_reg_limit;
        for i in 0..sub.n_args as usize {
            self.regs[sub.param_base as usize + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub.is_arrow { callee_obj.captured_this() } else { receiver };
        self.regs[255] = JsValue::undefined();
        for (name, reg) in &sub.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            if let Some(pos) = self.kernel.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }
        let _ = callee;

        let result = self.dispatch();

        // Restore execution state.
        self.regs = saved_regs;
        self.pc = saved_pc;
        self.bytecode = saved_bytecode;
        self.constants = saved_constants;
        self.active_reg_limit = saved_active_reg_limit;
        self.root_reg_limit = saved_root_reg_limit;
        self.try_stack = saved_try_stack;
        self.frames = saved_frames;
        self.exception_value = saved_exception;
        self.pending_exception = saved_pending;
        self.pending_error_kind = saved_pending_kind;
        self.for_in_iters = saved_for_in_iters;
        self.saved_bytecode_stack = saved_saved_bytecode_stack;
        self.saved_constants_stack = saved_saved_constants_stack;

        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_bytecode_frame(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue], construct_result_reg: Option<u8>,
        constructed_this: Option<JsValue>, new_target: JsValue, continuation: FrameContinuation,
    ) -> Result<(), String> {
        if !callee.is_object() {
            return Err("TypeError: CALL target is not callable".into());
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        if !obj.is_function() || obj.sub_module_index() == 0 {
            return Err("TypeError: CALL target is not callable".into());
        }
        let sub_idx = obj.sub_module_index() as usize - 1;
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "CALL: sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel.config.max_call_depth {
            return Err("RangeError: Maximum call stack size exceeded".into());
        }

        let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
        let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
        let sub_n_registers = self.sub_modules[sub_idx].n_registers;
        let sub_constants = self.sub_modules[sub_idx].constants.clone();
        let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
        let sub_is_arrow = self.sub_modules[sub_idx].is_arrow;
        let caller_reg_limit = self.active_reg_limit.max(1);
        let saved_regs = self.regs[..caller_reg_limit as usize].to_vec().into_boxed_slice();
        let saved_this = self.regs[254];
        let saved_new_target = self.regs[255];

        for i in 0..sub_n_args {
            self.regs[sub_param_base + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub_is_arrow { obj.captured_this() } else { this_value };
        self.regs[255] = new_target;

        let converted_sub_constants = self.convert_constants(&sub_constants)?;
        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
        self.saved_constants_stack.push(std::mem::take(&mut self.constants));

        let function_name = self.sub_modules[sub_idx]
            .function_name
            .as_deref()
            .map(|name| self.kernel.string_forge().intern(name).0)
            .unwrap_or(0);

        self.frames.push(CallFrame {
            return_addr: self.pc,
            function_name,
            caller_reg_limit,
            saved_regs,
            saved_this,
            saved_new_target,
            callee,
            construct_result_reg,
            constructed_this,
            is_derived_constructor: obj.is_derived_constructor(),
            continuation,
        });

        self.bytecode = sub_bytecode;
        self.constants = converted_sub_constants;
        for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map.clone() {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            if let Some(pos) = self.kernel.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.active_reg_limit = sub_n_registers.max(1);
        self.pc = 0;
        Ok(())
    }

    pub(crate) fn ordinary_get(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        self.ordinary_get_inner(obj, prop_name_si, receiver, None)
    }

    /// `ordinary_get` with an optional dispatch-loop target register.
    ///
    /// When `target_reg` is `Some(r)` and the property is a bytecode accessor getter,
    /// this method pushes a bytecode frame with `FrameContinuation::AccessorGet { target_reg: r }`
    /// and sets `self.accessor_frame_target_reg = Some(r)`. The caller must then return
    /// `Ok(JsValue::undefined())` — the real value will be stored by the `RETURN` handler.
    ///
    /// When `target_reg` is `None`, bytecode getters run inline via `call_function_sync`
    /// (same epoch, no sub-VM). Safe for callers outside the dispatch loop.
    pub(crate) fn ordinary_get_with_target(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue, target_reg: u8,
    ) -> Result<JsValue, String> {
        self.ordinary_get_inner(obj, prop_name_si, receiver, Some(target_reg))
    }

    fn ordinary_get_inner(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue, target_reg: Option<u8>,
    ) -> Result<JsValue, String> {
        let length_si = self.kernel.string_forge().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return Ok(JsValue::int(obj.prop_count() as i32));
        }
        if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    return if meta.get.is_undefined() {
                        Ok(JsValue::undefined())
                    } else if let Some(tr) = target_reg {
                        // Dispatch loop path: push bytecode frame, return sentinel.
                        let getter = meta.get;
                        let pushed = self.push_bytecode_getter_frame(getter, receiver, tr)?;
                        if pushed {
                            return Ok(JsValue::undefined()); // RETURN handler delivers value
                        }
                        // Native getter — value already written to regs[tr] by push_bytecode_getter_frame
                        Ok(self.regs[tr as usize])
                    } else {
                        self.call_function_sync(meta.get, receiver, &[])
                    };
                }
            }
            return Ok(obj.get_prop_at(pos));
        }
        let proto = obj.proto();
        if proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            return self.ordinary_get_inner(proto_obj, prop_name_si, receiver, target_reg);
        }
        Ok(JsValue::undefined())
    }

    /// Push a bytecode frame for a getter accessor, or call native getters inline.
    ///
    /// Returns `true` if a bytecode frame was pushed (dispatch loop must `continue`).
    /// Returns `false` if the getter was native (result already in `regs[target_reg]`).
    fn push_bytecode_getter_frame(
        &mut self, getter: JsValue, receiver: JsValue, target_reg: u8,
    ) -> Result<bool, String> {
        if !getter.is_object() {
            return Err("TypeError: getter is not callable".into());
        }
        let getter_obj = unsafe { &*getter.as_js_object_ptr() };
        if !getter_obj.is_function() {
            return Err("TypeError: getter is not callable".into());
        }

        if getter_obj.native_fn().is_some() {
            // Native getter — call inline, store result
            let result = self.call_function_sync(getter, receiver, &[])?;
            self.regs[target_reg as usize] = result;
            return Ok(false);
        }

        // Bytecode getter — push frame
        self.push_bytecode_frame(
            getter,
            receiver,
            &[],
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorGet { target_reg },
        )?;
        self.accessor_frame_target_reg = Some(target_reg);
        Ok(true)
    }

    fn inherited_property_meta(&self, obj: &JsObject, prop_name_si: u32) -> Option<oxide_types::object::PropMetaEntry> {
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self.get_own_property_slot(proto_obj, prop_name_si) {
                return proto_obj.prop_meta_at(pos);
            }
            proto = proto_obj.proto();
        }
        None
    }

    pub(crate) fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false)
    }

    /// `ordinary_set` variant for the dispatch loop: bytecode setters push a frame instead
    /// of spawning a sub-VM.
    pub(crate) fn ordinary_set_dispatch(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, true)
    }

    fn ordinary_set_inner(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.set.is_undefined() {
                        return self.raise_type_error("property has no setter");
                    }
                    return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
                }
                if !meta.attributes.writable() {
                    return self.raise_type_error("cannot assign to read-only property");
                }
            }
            obj.set_prop_at(pos, val);
            return Ok(());
        }

        if let Some(meta) = self.inherited_property_meta(obj, prop_name_si) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    return self.raise_type_error("property has no setter");
                }
                return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
            }
            if !meta.attributes.writable() {
                return self.raise_type_error("cannot assign to read-only property");
            }
        }

        self.set_or_create_prop_value(obj, prop_name_si, val);
        Ok(())
    }

    /// Call a setter — either inline (for native setters or sub-VM path) or via frame push
    /// (for bytecode setters inside the dispatch loop).
    fn call_or_push_setter(
        &mut self, setter: JsValue, receiver: JsValue, val: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        if !use_frame_push {
            self.call_function_sync(setter, receiver, &[val])?;
            return Ok(());
        }
        if !setter.is_object() {
            return self.raise_type_error("setter is not callable");
        }
        let setter_obj = unsafe { &*setter.as_js_object_ptr() };
        if !setter_obj.is_function() {
            return self.raise_type_error("setter is not callable");
        }
        if setter_obj.native_fn().is_some() {
            // Native setter — call inline
            self.call_function_sync(setter, receiver, &[val])?;
            return Ok(());
        }
        // Bytecode setter — push frame; result is discarded via AccessorSet
        self.push_bytecode_frame(
            setter,
            receiver,
            &[val],
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorSet,
        )?;
        Ok(())
    }

    pub(crate) fn read_member_prop(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let _ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        if obj.has_prop_meta() {
            return self.ordinary_get(obj, prop_name_si, receiver);
        }
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_slot = ext1;

        let val =
            if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_slot < obj.prop_vec_len() as u32 {
                obj.get_prop_at(cached_slot)
            } else if let Some(template) = self.kernel.prop_forge().get_template(obj.shape_id()) {
                if template.prop_name != prop_name_si {
                    self.ordinary_get(obj, prop_name_si, receiver)?
                } else if template.position < obj.prop_vec_len() as u32 {
                    self.write_ic_back(obj.shape_id(), template.position);
                    obj.get_prop_at(template.position)
                } else {
                    self.ordinary_get(obj, prop_name_si, receiver)?
                }
            } else {
                let resolved = self.ordinary_get(obj, prop_name_si, receiver)?;
                if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                    if !obj.is_accessor_meta(pos) {
                        self.write_ic_back(obj.shape_id(), pos);
                    }
                }
                resolved
            };
        Ok(val)
    }

    pub(crate) fn set_member_prop(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            if obj.has_prop_meta() {
                self.ordinary_set(obj, prop_name_si, val, receiver)?;
                return Ok(());
            }
            obj.set_prop_at(pos, val);
            // IC write-back: 3 extension words (shape_id + slot_index + reserved)
            self.write_ic_back(obj.shape_id(), pos);
        } else {
            self.ordinary_set(obj, prop_name_si, val, receiver)?;
        }
        Ok(())
    }

    pub(crate) fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_at(pos, val);
        } else {
            let new_shape_id = self.kernel.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(val);
            obj.bump_generation();
        }
    }

    pub(crate) fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        let pos = if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            pos
        } else {
            let new_shape_id = self.kernel.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined())
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable() {
                if current.is_accessor {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if current.attributes.enumerable() != attributes.enumerable() {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if !current.attributes.writable()
                    && (attributes.writable() || !coercion::same_value(obj.get_prop_at(pos), val))
                {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if current.attributes.configurable() != attributes.configurable() {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
            }
        }
        obj.set_prop_at(pos, val);
        obj.set_data_meta(pos, attributes);
        obj.bump_generation();
        Ok(())
    }

    pub(crate) fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        let pos = if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            pos
        } else {
            let new_shape_id = self.kernel.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined())
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable()
                && (!current.is_accessor
                    || current.attributes.enumerable() != attributes.enumerable()
                    || current.attributes.configurable() != attributes.configurable()
                    || current.get != get
                    || current.set != set)
            {
                return self.raise_type_error("cannot redefine non-configurable property");
            }
        }
        obj.set_prop_at(pos, JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, attributes);
        obj.bump_generation();
        Ok(())
    }

    pub(crate) fn write_ic_back(&mut self, shape_id: u32, slot_index: u32) {
        debug_assert!(self.pc >= 3, "IC write-back requires 3 extension words before pc");
        self.bytecode[self.pc - 3] = shape_id & 0x00FF_FFFF;
        self.bytecode[self.pc - 2] = slot_index;
        self.bytecode[self.pc - 1] = 0;
    }

    fn restore_frame(&mut self, frame: CallFrame) {
        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
            self.bytecode = saved_bc;
        }
        if let Some(saved_consts) = self.saved_constants_stack.pop() {
            self.constants = saved_consts;
        }
        let restore_len = frame.saved_regs.len();
        self.regs[..restore_len].copy_from_slice(&frame.saved_regs);
        self.regs[254] = frame.saved_this;
        self.regs[255] = frame.saved_new_target;
        self.active_reg_limit = frame.caller_reg_limit;
        self.pc = frame.return_addr;
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        self.clear_execution_state();
        self.active_reg_limit = self.root_reg_limit;
        self.clear_ic_caches();
        self.dispatch()
    }

    fn clear_ic_caches(&mut self) {
        let mut i = 0;
        while i < self.bytecode.len() {
            let op = opcode::opcode(self.bytecode[i]);
            if op.has_ic_ext_words() {
                if i + 3 < self.bytecode.len() {
                    self.bytecode[i + 1] = 0;
                    self.bytecode[i + 2] = 0;
                    self.bytecode[i + 3] = 0;
                }
                i += 4;
            } else {
                i += 1;
            }
        }
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.clear_execution_state();
        self.sub_modules = module.sub_modules.clone();
        self.constants = self.convert_constants(&module.constants)?;
        self.sub_module_constants = vec![Vec::new(); self.sub_modules.len()];
        self.bytecode = module.bytecode.clone();
        self.root_reg_limit = module.n_registers.max(1);
        self.active_reg_limit = self.root_reg_limit;

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            if let Some(pos) = self.kernel.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.dispatch()
    }

    /// Call a bytecode function from native code (D-09).
    /// Stub: sub_module storage not yet wired (plan 12.1-03).
    #[allow(dead_code)]
    pub fn call_bytecode_func(&mut self, _callback_obj: &JsObject, _args_regs: &[u8]) -> Result<JsValue, String> {
        Err("bytecode function calls not yet supported".into())
    }

    pub(crate) fn unwind(&mut self) -> Result<(), String> {
        while let Some(handler) = self.try_stack.pop() {
            while self.frames.len() > handler.frame_depth {
                if let Some(frame) = self.frames.pop() {
                    self.restore_frame(frame);
                }
            }
            if let Some(finally_pc) = handler.finally_pc {
                if self.pending_exception.is_none() {
                    self.pending_exception = self.exception_value.take();
                }
                self.try_stack.push(handler);
                self.pc = finally_pc;
                return Ok(());
            }
            if let Some(catch_pc) = handler.catch_pc {
                let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
                self.regs[0] = exc;
                self.pc = catch_pc;
                return Ok(());
            }
        }
        while let Some(frame) = self.frames.pop() {
            self.restore_frame(frame);
        }
        let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
        let kind_str = self.pending_error_kind.take().unwrap_or("Error");
        let exc_text = self.error_text(exc);
        let msg = if exc_text.starts_with(kind_str) {
            format!("uncaught {exc_text}")
        } else {
            format!("uncaught {kind_str}: {exc_text}")
        };
        Err(msg)
    }

    fn dispatch(&mut self) -> Result<JsValue, String> {
        let mut steps: u64 = 0;
        loop {
            steps += 1;
            if let Some(max_steps) = self.kernel.config.max_steps {
                if steps > max_steps {
                    return Err(format!("VM step limit exceeded at pc={}", self.pc));
                }
            }
            if self.pc >= self.bytecode.len() {
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

                OpCode::HALT => return Ok(self.regs[0]),

                OpCode::LOAD_CONST => {
                    self.dispatch_load_const(rd, instr)?;
                }

                OpCode::ADD => {
                    self.dispatch_add(rd, a, b);
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
                    let v = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(-v);
                }

                OpCode::BIT_AND => {
                    self.dispatch_bit_and(rd, a, b);
                }

                OpCode::BIT_OR => {
                    self.dispatch_bit_or(rd, a, b);
                }

                OpCode::BIT_XOR => {
                    self.dispatch_bit_xor(rd, a, b);
                }

                OpCode::SHL => {
                    self.dispatch_shl(rd, a, b);
                }

                OpCode::SHR => {
                    self.dispatch_shr(rd, a, b);
                }

                OpCode::USHR => {
                    self.dispatch_ushr(rd, a, b);
                }

                OpCode::BIT_NOT => {
                    self.dispatch_bit_not(rd, a);
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
                    let v = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(v);
                }

                OpCode::JMP => {
                    let offset = opcode::offset16(instr) as isize;
                    self.pc = ((self.pc as isize) + offset - 1) as usize;
                }

                OpCode::JMP_IF_FALSE => {
                    let cond = coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if !cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::JMP_IF_TRUE => {
                    let cond = coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::LOAD_VAR => {
                    if a == 254
                        && self.frames.last().map(|frame| frame.is_derived_constructor).unwrap_or(false)
                        && self.regs[a].is_undefined()
                    {
                        throw_err!(self, ReferenceError, "must call super constructor before using 'this'");
                    }
                    self.regs[rd] = self.regs[a];
                }

                OpCode::STORE_VAR => {
                    if b != 0 {
                        // const guard: check if already initialized
                        if !self.regs[rd].is_undefined() {
                            throw_err!(self, TypeError, "Assignment to constant variable");
                        }
                    }
                    self.regs[rd] = self.regs[a];
                }

                OpCode::CALL => {
                    let callee_reg = rd;
                    let this_reg = a as u8;
                    let first_arg_reg = b as u8;

                    let callee = self.regs[callee_reg];

                    if callee.is_object() {
                        let obj_ptr = callee.as_js_object_ptr();
                        if !obj_ptr.is_null() {
                            let obj = unsafe { &*obj_ptr };
                            if obj.is_function() {
                                if obj.is_class_constructor() {
                                    throw_err!(self, TypeError, "class constructor cannot be invoked without 'new'");
                                }
                                let ext = self.bytecode[self.pc];
                                self.pc += 1;
                                let arg_count = (ext & 0xFF) as usize;

                                if obj.native_fn().is_some() {
                                    match self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count) {
                                        Ok(()) => continue,
                                        Err(e) => return Err(e),
                                    }
                                } else if obj.sub_module_index() > 0 {
                                    let args: Vec<JsValue> = (0..arg_count)
                                        .map(|i| self.regs[first_arg_reg.wrapping_add(i as u8) as usize])
                                        .collect();
                                    self.push_bytecode_frame(
                                        callee,
                                        self.regs[this_reg as usize],
                                        &args,
                                        None,
                                        None,
                                        JsValue::undefined(),
                                        FrameContinuation::None,
                                    )?;
                                    continue;
                                }
                            }
                        }
                    }

                    throw_err!(self, TypeError, "CALL target is not callable");
                }

                OpCode::CALL_NATIVE => {
                    let callee_reg = rd;
                    let this_reg = a as u8;
                    let first_arg_reg = b as u8;

                    let callee = self.regs[callee_reg];

                    if !callee.is_object() {
                        throw_err!(self, TypeError, "CALL_NATIVE target is not an object");
                    }
                    let obj_ptr = callee.as_js_object_ptr();
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "CALL_NATIVE target is null");
                    }
                    let obj = unsafe { &*obj_ptr };
                    if !obj.is_function() || obj.native_fn().is_none() {
                        throw_err!(self, TypeError, "CALL_NATIVE target is not a native function");
                    }

                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)?;
                }

                OpCode::NEW_EXPRESSION => {
                    let constructor_reg = a;
                    let first_arg_reg = b as u8;

                    let constructor = self.regs[constructor_reg];
                    if !constructor.is_object() {
                        throw_err!(self, TypeError, "NEW_EXPRESSION: constructor is not an object");
                    }
                    let ctor_ptr = constructor.as_js_object_ptr();
                    if ctor_ptr.is_null() {
                        throw_err!(self, TypeError, "NEW_EXPRESSION: constructor is null");
                    }
                    let ctor_obj = unsafe { &*ctor_ptr };
                    if !ctor_obj.is_function() {
                        throw_err!(self, TypeError, "NEW_EXPRESSION: constructor is not a function");
                    }
                    // Arrow functions cannot be used as constructors (D-03)
                    if ctor_obj.is_arrow() {
                        throw_err!(self, TypeError, "arrow functions cannot be used as constructors");
                    }

                    // Read extension word for arg_count
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    // Create new empty object
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let new_obj = self
                        .epoch
                        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));

                    // Look up constructor.prototype and set as proto of new object
                    let proto_si = self.kernel.string_forge().intern("prototype").0;
                    if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
                        if proto_val.is_object() {
                            let new_obj_mut = unsafe { &mut *new_obj };
                            let proto_obj_ptr = proto_val.as_js_object_ptr();
                            let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
                        }
                    }

                    // If constructor has native_fn, call it with this=new_obj
                    if ctor_obj.native_fn().is_some() {
                        let new_obj_val = JsValue::object(new_obj as *mut u8);
                        self.regs[255] = new_obj_val;

                        let mut args_buf = [0u8; 257];
                        args_buf[0] = 255u8;
                        for i in 0..arg_count.min(256) {
                            args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
                        }
                        let args_slice = &args_buf[..arg_count + 1];

                        // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
                        // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
                        let func: NativeFn = unsafe { native_fn_ptr_to_fn(ctor_obj.native_fn().unwrap()) };
                        self.regs[254] = constructor;
                        match func(self, args_slice) {
                            NativeResult::Ok(val) => {
                                if val.is_object() {
                                    self.regs[rd] = val;
                                } else {
                                    self.regs[rd] = new_obj_val;
                                }
                            }
                            NativeResult::Err(err_val) => {
                                self.exception_value = Some(err_val);
                                self.pending_error_kind = Some("Error");
                                match self.unwind() {
                                    Ok(()) => continue,
                                    Err(e) => return Err(e),
                                }
                            }
                            NativeResult::TailCall { .. } => {
                                throw_err!(self, TypeError, "constructor tail call not supported");
                            }
                        }
                    } else if ctor_obj.sub_module_index() > 0 {
                        let sub_idx = ctor_obj.sub_module_index() as usize - 1;
                        if sub_idx >= self.sub_modules.len() {
                            return Err(format!(
                                "NEW_EXPRESSION: sub_module_index {} out of bounds (max {})",
                                sub_idx,
                                self.sub_modules.len()
                            ));
                        }

                        if self.frames.len() >= self.kernel.config.max_call_depth {
                            return Err("RangeError: Maximum call stack size exceeded".into());
                        }

                        let new_obj_val = JsValue::object(new_obj as *mut u8);
                        let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
                        let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
                        let sub_n_registers = self.sub_modules[sub_idx].n_registers;
                        let sub_constants = self.sub_modules[sub_idx].constants.clone();
                        let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
                        let caller_reg_limit = self.active_reg_limit.max(1);
                        let saved_regs = self.regs[..caller_reg_limit as usize].to_vec().into_boxed_slice();
                        let saved_this = self.regs[254];
                        let saved_new_target = self.regs[255];

                        for i in 0..sub_n_args {
                            let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                            self.regs[sub_param_base + i] = self.regs[src_reg];
                        }
                        self.regs[254] = if ctor_obj.is_derived_constructor() {
                            JsValue::undefined()
                        } else {
                            new_obj_val
                        };
                        self.regs[255] = constructor;

                        let converted_sub_constants = self.convert_constants(&sub_constants)?;

                        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
                        self.saved_constants_stack.push(std::mem::take(&mut self.constants));

                        self.frames.push(CallFrame {
                            return_addr: self.pc,
                            function_name: self.sub_modules[sub_idx]
                                .function_name
                                .as_deref()
                                .map(|name| self.kernel.string_forge().intern(name).0)
                                .unwrap_or(0),
                            caller_reg_limit,
                            saved_regs,
                            saved_this,
                            saved_new_target,
                            callee: constructor,
                            construct_result_reg: Some(rd as u8),
                            constructed_this: Some(new_obj_val),
                            is_derived_constructor: ctor_obj.is_derived_constructor(),
                            continuation: FrameContinuation::None,
                        });

                        self.bytecode = sub_bytecode;
                        self.constants = converted_sub_constants;

                        for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                            let si = self.kernel.string_forge().intern(name.as_str()).0;
                            let global = self.kernel.global_object();
                            if let Some(pos) = self.kernel.shape_forge().lookup_position(global.shape_id(), si) {
                                self.regs[*reg as usize] = global.get_prop_at(pos);
                            }
                        }

                        self.active_reg_limit = sub_n_registers.max(1);
                        self.pc = 0;
                        continue;
                    } else {
                        let error = crate::builtins::error::create_error(
                            self,
                            "NEW_EXPRESSION: bytecode constructors not yet supported",
                        );
                        self.exception_value = Some(error);
                        self.pending_error_kind = Some("Error");
                        match self.unwind() {
                            Ok(()) => continue,
                            Err(e) => return Err(e),
                        }
                    }
                }

                OpCode::SUPER_CALL => {
                    let first_arg_reg = a as u8;
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    let Some(frame) = self.frames.last() else {
                        throw_err!(self, ReferenceError, "super() used outside class constructor");
                    };
                    if !frame.is_derived_constructor {
                        throw_err!(self, ReferenceError, "super() used outside derived constructor");
                    }
                    if !self.regs[254].is_undefined() {
                        throw_err!(self, ReferenceError, "super() called more than once");
                    }
                    let Some(derived_this) = frame.constructed_this else {
                        throw_err!(self, ReferenceError, "super() without derived this");
                    };

                    let new_target = self.regs[255];
                    if !new_target.is_object() {
                        throw_err!(self, TypeError, "super() new.target is not an object");
                    }
                    let new_target_obj = unsafe { &*new_target.as_js_object_ptr() };
                    let super_ctor = new_target_obj.proto();
                    if !super_ctor.is_object() {
                        throw_err!(self, TypeError, "super constructor is not an object");
                    }
                    let super_obj = unsafe { &*super_ctor.as_js_object_ptr() };
                    if !super_obj.is_function() {
                        throw_err!(self, TypeError, "super constructor is not a function");
                    }

                    if super_obj.native_fn().is_some() {
                        self.regs[253] = derived_this;
                        let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, 253);
                        // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
                        // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
                        let func: NativeFn = unsafe { native_fn_ptr_to_fn(super_obj.native_fn().unwrap()) };
                        match func(self, &args_buf[..len]) {
                            NativeResult::Ok(val) => {
                                self.regs[254] = if val.is_object() { val } else { derived_this };
                                self.regs[rd] = self.regs[254];
                            }
                            NativeResult::Err(err_val) => {
                                self.exception_value = Some(err_val);
                                self.pending_error_kind = Some("Error");
                                match self.unwind() {
                                    Ok(()) => continue,
                                    Err(e) => return Err(e),
                                }
                            }
                            NativeResult::TailCall { .. } => {
                                throw_err!(self, TypeError, "super constructor tail call not supported");
                            }
                        }
                    } else if super_obj.sub_module_index() > 0 {
                        let sub_idx = super_obj.sub_module_index() as usize - 1;
                        if sub_idx >= self.sub_modules.len() {
                            return Err(format!(
                                "SUPER_CALL: sub_module_index {} out of bounds (max {})",
                                sub_idx,
                                self.sub_modules.len()
                            ));
                        }
                        if self.frames.len() >= self.kernel.config.max_call_depth {
                            return Err("RangeError: Maximum call stack size exceeded".into());
                        }

                        let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
                        let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
                        let sub_n_registers = self.sub_modules[sub_idx].n_registers;
                        let sub_constants = self.sub_modules[sub_idx].constants.clone();
                        let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
                        let caller_reg_limit = self.active_reg_limit.max(1);
                        let saved_regs = self.regs[..caller_reg_limit as usize].to_vec().into_boxed_slice();
                        let saved_this = self.regs[254];
                        let saved_new_target = self.regs[255];

                        for i in 0..sub_n_args {
                            let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                            self.regs[sub_param_base + i] = self.regs[src_reg];
                        }
                        self.regs[254] = derived_this;
                        self.regs[255] = new_target;

                        let converted_sub_constants = self.convert_constants(&sub_constants)?;
                        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
                        self.saved_constants_stack.push(std::mem::take(&mut self.constants));
                        self.frames.push(CallFrame {
                            return_addr: self.pc,
                            function_name: self.sub_modules[sub_idx]
                                .function_name
                                .as_deref()
                                .map(|name| self.kernel.string_forge().intern(name).0)
                                .unwrap_or(0),
                            caller_reg_limit,
                            saved_regs,
                            saved_this,
                            saved_new_target,
                            callee: super_ctor,
                            construct_result_reg: Some(254),
                            constructed_this: Some(derived_this),
                            is_derived_constructor: super_obj.is_derived_constructor(),
                            continuation: FrameContinuation::None,
                        });

                        self.bytecode = sub_bytecode;
                        self.constants = converted_sub_constants;
                        for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                            let si = self.kernel.string_forge().intern(name.as_str()).0;
                            let global = self.kernel.global_object();
                            if let Some(pos) = self.kernel.shape_forge().lookup_position(global.shape_id(), si) {
                                self.regs[*reg as usize] = global.get_prop_at(pos);
                            }
                        }
                        self.active_reg_limit = sub_n_registers.max(1);
                        self.pc = 0;
                        continue;
                    } else {
                        throw_err!(self, TypeError, "super constructor is not callable");
                    }
                }

                OpCode::SUPER_GET_PROP | OpCode::SUPER_STATIC_GET_PROP => {
                    let key_val = self.regs[b];
                    let prop_name_si = self.property_key_si(key_val);
                    let Some(frame) = self.frames.last() else {
                        throw_err!(self, ReferenceError, "super property used outside function");
                    };
                    if !frame.callee.is_object() {
                        throw_err!(self, ReferenceError, "super property has no home object");
                    }
                    let callee_obj = unsafe { &*frame.callee.as_js_object_ptr() };
                    let home_object = callee_obj.home_object();
                    if !home_object.is_object() {
                        throw_err!(self, ReferenceError, "super property has no home object");
                    }
                    let home_obj = unsafe { &*home_object.as_js_object_ptr() };
                    let super_base = home_obj.proto();
                    if !super_base.is_object() {
                        self.regs[rd] = JsValue::undefined();
                    } else {
                        let super_obj = unsafe { &*super_base.as_js_object_ptr() };
                        let val = self.ordinary_get_with_target(super_obj, prop_name_si, self.regs[a], rd as u8)?;
                        if self.accessor_frame_target_reg.take().is_none() {
                            self.regs[rd] = val;
                        }
                    }
                }

                OpCode::SET_HOME_OBJECT => {
                    let func_val = self.regs[rd];
                    let home_val = self.regs[a];
                    if !func_val.is_object() || !home_val.is_object() {
                        throw_err!(self, TypeError, "SET_HOME_OBJECT expects function and object");
                    }
                    let func_obj = unsafe { &mut *func_val.as_js_object_ptr() };
                    if !func_obj.is_function() {
                        throw_err!(self, TypeError, "SET_HOME_OBJECT target is not a function");
                    }
                    func_obj.set_home_object(home_val);
                }

                OpCode::DEFINE_ACCESSOR => {
                    let prop_idx = self.bytecode[self.pc] as usize;
                    self.pc += 1;
                    if prop_idx >= self.constants.len() {
                        throw_err!(self, TypeError, "DEFINE_ACCESSOR constant index out of bounds");
                    }
                    let obj_val = self.regs[rd];
                    if !obj_val.is_object() {
                        throw_err!(self, TypeError, "DEFINE_ACCESSOR target is not object");
                    }
                    let prop_name_si = self.property_key_si(self.constants[prop_idx]);
                    let getter = self.regs[a];
                    let setter = self.regs[b];
                    let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
                    let existing = self
                        .get_own_property_slot(obj, prop_name_si)
                        .and_then(|pos| obj.prop_meta_at(pos))
                        .filter(|meta| meta.is_accessor);
                    let get = if getter.is_undefined() {
                        existing.map(|meta| meta.get).unwrap_or(JsValue::undefined())
                    } else {
                        getter
                    };
                    let set = if setter.is_undefined() {
                        existing.map(|meta| meta.set).unwrap_or(JsValue::undefined())
                    } else {
                        setter
                    };
                    self.define_accessor_property(obj, prop_name_si, get, set, PropAttributes::DEFAULT_DATA)?;
                }

                OpCode::RETURN => {
                    let result = self.regs[rd];
                    if let Some(frame) = self.frames.pop() {
                        let construct_result_reg = frame.construct_result_reg;
                        let constructed_this = frame.constructed_this;
                        let is_derived_constructor = frame.is_derived_constructor;
                        let continuation = frame.continuation;
                        let callee_this = self.regs[254];
                        self.restore_frame(frame);
                        if let (Some(target_reg), Some(constructed_this)) = (construct_result_reg, constructed_this) {
                            if is_derived_constructor && result.is_undefined() && callee_this.is_undefined() {
                                throw_err!(self, ReferenceError, "derived constructor must call super()");
                            }
                            self.regs[target_reg as usize] = if result.is_object() { result } else { constructed_this };
                            self.regs[0] = self.regs[target_reg as usize];
                        } else {
                            match continuation {
                                FrameContinuation::None => {
                                    self.regs[0] = result;
                                }
                                FrameContinuation::AccessorGet { target_reg } => {
                                    self.regs[target_reg as usize] = result;
                                }
                                FrameContinuation::AccessorSet => {}
                            }
                        }
                    } else {
                        return Ok(result);
                    }
                }

                OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::GET_PROP
                | OpCode::SET_PROP
                | OpCode::GET_PROP_DYNAMIC
                | OpCode::SET_PROP_DYNAMIC
                | OpCode::SET_ELEM => {
                    self.dispatch_property_op(op, rd, a, b)?;
                }

                OpCode::NEW_OBJECT => {
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let obj = self
                        .epoch
                        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::NEW_ARRAY => {
                    let proto_ptr = self.kernel.builtin_world().array_proto.as_ptr() as *mut JsObject;
                    let n = opcode::imm16(instr) as usize;
                    let bump = self.epoch.bump();
                    let obj = self.epoch.alloc(JsObject::new_array(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                        n,
                        bump,
                    ));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::COMPOUND_ADD => {
                    let lhs = self.regs[rd];
                    let rhs = self.regs[a];
                    if lhs.is_string() || rhs.is_string() {
                        let ls = coercion::to_string(self.kernel.string_forge().as_ref(), lhs);
                        let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
                        let concat = format!("{ls}{rs}");
                        self.regs[rd] = self.intern(&concat);
                    } else {
                        let ln = coercion::to_number(lhs, self.kernel.string_forge().as_ref());
                        let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
                        self.regs[rd] = JsValue::float(ln + rn);
                    }
                }

                OpCode::COMPOUND_SUB => {
                    compound_arith!(self, rd, a, -);
                }

                OpCode::COMPOUND_MUL => {
                    compound_arith!(self, rd, a, *);
                }

                OpCode::COMPOUND_DIV => {
                    compound_arith!(self, rd, a, /);
                }

                OpCode::COMPOUND_MOD => {
                    compound_arith!(self, rd, a, %);
                }

                OpCode::COMPOUND_EXP => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l.powf(r));
                }

                OpCode::COMPOUND_AND => {
                    self.dispatch_compound_bit_and(rd, a);
                }

                OpCode::COMPOUND_OR => {
                    self.dispatch_compound_bit_or(rd, a);
                }

                OpCode::COMPOUND_XOR => {
                    self.dispatch_compound_bit_xor(rd, a);
                }

                OpCode::COMPOUND_SHL => {
                    self.dispatch_compound_shl(rd, a);
                }

                OpCode::COMPOUND_SHR => {
                    self.dispatch_compound_shr(rd, a);
                }

                OpCode::COMPOUND_USHR => {
                    self.dispatch_compound_ushr(rd, a);
                }

                OpCode::TYPEOF => {
                    self.dispatch_typeof(rd, a);
                }

                OpCode::VOID => {
                    self.regs[rd] = JsValue::undefined();
                }

                OpCode::TEMPLATE_STR => {
                    // Read header ext word: (segment_count << 16) | (total_len_hint & 0xFFFF)
                    let header = self.bytecode[self.pc];
                    self.pc += 1;
                    let segment_count = (header >> 16) as usize;
                    let len_hint = (header & 0xFFFF) as usize;

                    // Build result string
                    let mut result = String::with_capacity(len_hint.max(16));
                    for _ in 0..segment_count {
                        let seg = self.bytecode[self.pc];
                        self.pc += 1;
                        if (seg >> 31) == 1 {
                            // Expression: register value
                            let reg = (seg & 0x7F) as u8;
                            let val = self.regs[reg as usize];
                            let s = if val.is_string() {
                                self.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default()
                            } else {
                                format!("{}", val)
                            };
                            result.push_str(&s);
                        } else {
                            // Quasi: constant string
                            let const_idx = (seg & 0x7FFF_FFFF) as usize;
                            if const_idx < self.constants.len() {
                                let val = self.constants[const_idx];
                                if val.is_string() {
                                    let s =
                                        self.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default();
                                    result.push_str(&s);
                                }
                            }
                        }
                    }
                    let si = self.kernel.string_forge().intern(&result).0;
                    self.regs[rd] = JsValue::string(si, 0);
                }

                OpCode::DELETE_PROP_STATIC => {
                    let _ = self.bytecode[self.pc];
                    self.pc += 1;
                    throw_err!(self, TypeError, "property deletion not supported");
                }

                OpCode::DELETE_PROP_DYNAMIC => {
                    let _ = self.regs[a];
                    let _ = self.regs[b];
                    throw_err!(self, TypeError, "property deletion not supported");
                }

                OpCode::INSTANCEOF => {
                    let lhs_val = self.regs[a];
                    let rhs_val = self.regs[b];

                    if !rhs_val.is_object() {
                        throw_err!(self, TypeError, "INSTANCEOF right-hand side is not callable");
                    }
                    if !lhs_val.is_object() {
                        self.regs[rd] = JsValue::bool(false);
                        continue;
                    }

                    let rhs_obj = unsafe { &*rhs_val.as_js_object_ptr() };
                    let proto_si = self.kernel.string_forge().intern("prototype").0;
                    let ctor_proto = self.resolve_property(rhs_obj, proto_si);

                    let ctor_proto_ptr = match ctor_proto {
                        Some(v) if v.is_object() => v.as_js_object_ptr(),
                        _ => {
                            self.regs[rd] = JsValue::bool(false);
                            continue;
                        }
                    };

                    let mut proto = unsafe { &*lhs_val.as_js_object_ptr() }.proto();
                    loop {
                        if !proto.is_object() {
                            self.regs[rd] = JsValue::bool(false);
                            break;
                        }
                        let proto_ptr = proto.as_js_object_ptr();
                        if proto_ptr == ctor_proto_ptr {
                            self.regs[rd] = JsValue::bool(true);
                            break;
                        }
                        proto = unsafe { &*proto_ptr }.proto();
                    }
                }

                OpCode::IN => {
                    let key_val = self.regs[a];
                    let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "IN right-hand side is not an object");
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.property_key_si(key_val);
                    let found = self.resolve_property(obj, prop_name_si).is_some();
                    self.regs[rd] = JsValue::bool(found);
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

                OpCode::INC_PRE => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let result = JsValue::float(n + 1.0);
                    self.regs[rd] = result;
                    self.regs[a] = result;
                }

                OpCode::INC_POST => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    self.regs[a] = JsValue::float(n);
                    self.regs[rd] = JsValue::float(n + 1.0);
                }

                OpCode::DEC_PRE => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let result = JsValue::float(n - 1.0);
                    self.regs[rd] = result;
                    self.regs[a] = result;
                }

                OpCode::DEC_POST => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    self.regs[a] = JsValue::float(n);
                    self.regs[rd] = JsValue::float(n - 1.0);
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
                    let obj_val = self.regs[a];
                    if !obj_val.is_object() {
                        throw_err!(self, TypeError, "for-in right-hand side is not an object");
                    }

                    let mut keys_vec = bumpalo::collections::Vec::new_in(self.epoch.bump());
                    let mut seen = std::collections::HashSet::new();
                    let mut current = obj_val;

                    loop {
                        if !current.is_object() {
                            break;
                        }
                        let cur = unsafe { &*current.as_js_object_ptr() };
                        let mut cursor = Some(cur.shape_id());
                        while let Some(id) = cursor {
                            if id == EMPTY_SHAPE_ID {
                                break;
                            }
                            if let Some(shape) = self.kernel.shape_forge().get_shape(id) {
                                if shape.property_name != u32::MAX && seen.insert(shape.property_name) {
                                    let enumerable = self
                                        .kernel
                                        .shape_forge()
                                        .lookup_position(cur.shape_id(), shape.property_name)
                                        .and_then(|pos| cur.prop_meta_at(pos))
                                        .map(|meta| meta.attributes.enumerable())
                                        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                                    if enumerable {
                                        let hash =
                                            self.kernel.string_forge().get_hash(shape.property_name).unwrap_or(0);
                                        keys_vec.push(JsValue::string(shape.property_name, hash));
                                    }
                                }
                                cursor = shape.parent;
                            } else {
                                break;
                            }
                        }
                        current = cur.proto();
                    }

                    let iter = self.epoch.alloc(ForInIter { keys: keys_vec, index: 0 });
                    // SAFETY: ForInIter is arena-allocated and valid until `epoch.reset()`;
                    // dispatch clears `for_in_iters` before resetting the epoch.
                    self.for_in_iters.push(iter.cast::<ForInIter<'static>>());
                }

                OpCode::FOR_IN_NEXT => {
                    let iter_ptr = self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut());
                    if iter_ptr.is_null() {
                        return Err("FOR_IN_NEXT without active iterator".into());
                    }
                    let iter = unsafe { &mut *iter_ptr };
                    if iter.index < iter.keys.len() {
                        self.regs[rd] = iter.keys[iter.index];
                        iter.index += 1;
                    } else {
                        self.regs[rd] = JsValue::undefined();
                    }
                }

                OpCode::FOR_IN_DONE => {
                    let iter_ptr = self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut());
                    if iter_ptr.is_null() {
                        self.regs[rd] = JsValue::bool(true);
                    } else {
                        let iter = unsafe { &*iter_ptr };
                        self.regs[rd] = JsValue::bool(iter.index >= iter.keys.len());
                    }
                }

                OpCode::FOR_IN_CLEANUP => {
                    self.for_in_iters.pop();
                }

                OpCode::FOR_OF_INIT => {
                    return Err("opcode FOR_OF_INIT not yet implemented".into());
                }

                OpCode::FOR_OF_NEXT => {
                    return Err("opcode FOR_OF_NEXT not yet implemented".into());
                }

                OpCode::FOR_OF_DONE => {
                    return Err("opcode FOR_OF_DONE not yet implemented".into());
                }

                OpCode::FOR_OF_CLOSE => {
                    return Err("opcode FOR_OF_CLOSE not yet implemented".into());
                }

                OpCode::THROW => {
                    let exc_value = self.regs[rd];
                    self.exception_value = Some(exc_value);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc_value));
                    match self.unwind() {
                        Ok(()) => continue,
                        Err(e) => return Err(e),
                    }
                }

                OpCode::TRY_BEGIN => {
                    let offset = opcode::offset16(instr) as isize;
                    let catch_pc = if offset == 0 {
                        None
                    } else {
                        Some(((self.pc as isize) + offset - 1) as usize)
                    };
                    self.try_stack.push(TryHandler {
                        catch_pc,
                        finally_pc: None,
                        frame_depth: self.frames.len(),
                    });
                }

                OpCode::TRY_END => {
                    self.try_stack.pop();
                }

                OpCode::TRY_FINALLY_BEGIN => {
                    let offset = opcode::offset16(instr) as isize;
                    let finally_pc = ((self.pc as isize) + offset - 1) as usize;
                    self.try_stack.push(TryHandler {
                        catch_pc: None,
                        finally_pc: Some(finally_pc),
                        frame_depth: self.frames.len(),
                    });
                }

                OpCode::TRY_FINALLY_END => {
                    self.try_stack.pop();
                    if self.pending_exception.is_some() && self.exception_value.is_none() {
                        self.exception_value = self.pending_exception.take();
                        match self.unwind() {
                            Ok(()) => continue,
                            Err(e) => return Err(e),
                        }
                    }
                    self.pending_exception = None;
                }

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
    use crate::native::NativeResult;
    use oxide_compiler::module::{CompiledModule, Constant};
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
        let marker_si = vm.kernel.string_forge().intern("marker").0;
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        NativeResult::Ok(vm.resolve_property(obj, marker_si).unwrap_or(JsValue::undefined()))
    }

    fn native_set_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        let value = vm.reg(args[1]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel.string_forge().intern("marker").0;
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, marker_si, value);
        NativeResult::Ok(JsValue::undefined())
    }

    fn native_function(vm: &mut Vm, f: crate::native::NativeFn) -> JsValue {
        let proto = vm.kernel.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let mut obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: f is a NativeFn fn-item; valid to store as NativeFnPtr.
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(f as *const ()) }));
        JsValue::object(vm.epoch.alloc(obj) as *mut u8)
    }

    fn plain_object(vm: &mut Vm) -> JsValue {
        let proto = vm.kernel.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        JsValue::object(vm.epoch.alloc(obj) as *mut u8)
    }

    fn add_accessor(vm: &mut Vm, obj_val: JsValue, name: &str, get: JsValue, set: JsValue) {
        let si = vm.kernel.string_forge().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let shape_id = vm.kernel.shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape_id);
        let pos = obj.push_prop(JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, PropAttributes::DEFAULT_DATA);
        obj.bump_generation();
    }

    fn set_data(vm: &mut Vm, obj_val: JsValue, name: &str, val: JsValue) {
        let si = vm.kernel.string_forge().intern(name).0;
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
            saved_regs: vec![JsValue::undefined()].into_boxed_slice(),
            saved_this: JsValue::undefined(),
            saved_new_target: JsValue::undefined(),
            callee: JsValue::undefined(),
            construct_result_reg: None,
            constructed_this: None,
            is_derived_constructor: false,
            continuation: super::FrameContinuation::None,
        });
        vm.for_in_iters.push(std::ptr::dangling_mut::<super::ForInIter<'static>>());
        vm.for_of_iters.push(std::ptr::dangling_mut::<u8>());
        vm.saved_bytecode_stack
            .push(vec![opcode::encode(opcode::OpCode::HALT, 0, 0, 0)]);
        vm.saved_constants_stack.push(vec![JsValue::int(1)]);
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
        assert!(vm.for_in_iters.is_empty());
        assert!(vm.for_of_iters.is_empty());
        assert!(vm.saved_bytecode_stack.is_empty());
        assert!(vm.saved_constants_stack.is_empty());
        assert!(vm.try_stack.is_empty());
        assert!(vm.exception_value.is_none());
        assert!(vm.pending_exception.is_none());
        assert!(vm.pending_error_kind.is_none());
        assert!(vm.bytecode.is_empty());
        assert!(vm.constants.is_empty());
    }

    #[test]
    fn for_of_opcodes_fail_explicitly() {
        for (op, expected) in [
            (opcode::OpCode::FOR_OF_INIT, "opcode FOR_OF_INIT not yet implemented"),
            (opcode::OpCode::FOR_OF_NEXT, "opcode FOR_OF_NEXT not yet implemented"),
            (opcode::OpCode::FOR_OF_DONE, "opcode FOR_OF_DONE not yet implemented"),
            (opcode::OpCode::FOR_OF_CLOSE, "opcode FOR_OF_CLOSE not yet implemented"),
        ] {
            let module = CompiledModule {
                bytecode: vec![opcode::encode(op, 0, 0, 0), opcode::encode(opcode::OpCode::HALT, 0, 0, 0)],
                n_registers: 1,
                ..CompiledModule::new()
            };
            let mut vm = Vm::new();
            let err = vm.run(&module).expect_err("FOR_OF opcode should fail explicitly");
            assert_eq!(err, expected);
        }
    }

    #[test]
    fn invalid_regexp_constant_fails_explicitly() {
        let module = CompiledModule {
            constants: vec![Constant::RegExp("[".into(), "".into())],
            bytecode: vec![opcode::encode(opcode::OpCode::HALT, 0, 0, 0)],
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        let err = vm.run(&module).expect_err("invalid RegExp constant should fail explicitly");
        assert!(err.contains("SyntaxError: Invalid regular expression"), "unexpected error: {err}");
    }

    #[test]
    fn invalid_regexp_constant_in_submodule_fails_explicitly() {
        let submodule = CompiledModule {
            constants: vec![Constant::RegExp("[".into(), "".into())],
            bytecode: vec![opcode::encode(opcode::OpCode::RETURN, 0, 0, 0)],
            n_registers: 1,
            ..CompiledModule::new()
        };
        let module = CompiledModule {
            constants: vec![Constant::BytecodeFunc(1), Constant::Undefined],
            bytecode: vec![
                opcode::encode(opcode::OpCode::LOAD_CONST, 0, 0, 0),
                opcode::encode(opcode::OpCode::LOAD_CONST, 1, 1, 0),
                opcode::encode(opcode::OpCode::CALL, 0, 1, 0),
                0,
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ],
            n_registers: 2,
            sub_modules: vec![submodule],
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        let err = vm
            .run(&module)
            .expect_err("invalid submodule RegExp constant should fail explicitly");
        assert!(err.contains("SyntaxError: Invalid regular expression"), "unexpected error: {err}");
    }

    #[test]
    fn write_ic_back_updates_three_extension_words() {
        let mut vm = Vm::new();
        vm.bytecode = vec![0, 0, 0];
        vm.pc = 3;
        vm.write_ic_back(0x1234_5678, 7);
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

        let x_si = vm.kernel.string_forge().intern("x").0;
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

        let x_si = vm.kernel.string_forge().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.ordinary_set(obj, x_si, JsValue::int(9), obj_val).expect("setter");

        let marker_si = vm.kernel.string_forge().intern("marker").0;
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

        let x_si = vm.kernel.string_forge().intern("x").0;
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

        let x_si = vm.kernel.string_forge().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(12), child_val).expect("setter");

        let marker_si = vm.kernel.string_forge().intern("marker").0;
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

        let x_si = vm.kernel.string_forge().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(15), child_val).expect("setter");

        let marker_si = vm.kernel.string_forge().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(15)));
    }

    #[test]
    fn ordinary_data_property_still_reads_and_writes_without_meta() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        set_data(&mut vm, obj_val, "x", JsValue::int(1));

        let x_si = vm.kernel.string_forge().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        assert!(!obj.has_prop_meta());
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(1));
        vm.ordinary_set(obj, x_si, JsValue::int(2), obj_val).expect("set");
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(2));
    }
}
