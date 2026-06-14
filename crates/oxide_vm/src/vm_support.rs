#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;

use crate::bindings;
use crate::vm::{native_fn_ptr_to_fn, Vm};
use oxide_kernel::kernel::{KernelConfig, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::error::JsError;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    pub fn new() -> Self {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: smallvec::SmallVec::new(),
            for_in_iters: Vec::new(),
            kernel_core: core,
            session,
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
            symbol_registry: std::collections::HashMap::new(),
            for_of_iters: Vec::new(),
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
        }
    }

    pub fn with_kernel_core(core: Arc<KernelCore>) -> Self {
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: smallvec::SmallVec::new(),
            for_in_iters: Vec::new(),
            kernel_core: core,
            session,
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
            symbol_registry: std::collections::HashMap::new(),
            for_of_iters: Vec::new(),
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
        }
    }

    pub fn full_reset(&mut self) {
        let mut new_session = KernelSession::new(&self.kernel_core);
        bindings::init_kernel_builtins(&self.kernel_core, &mut new_session);
        self.object_prototype = P::clone(&new_session.builtin_world().object_proto);
        self.session = new_session;
        self.clear_execution_state();
        self.bytecode.clear();
        self.constants.clear();
        self.epoch.reset();
        self.interned_strings.clear();
        self.symbol_counter = 0;
        self.symbol_descriptions.clear();
        self.symbol_registry.clear();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    pub(crate) fn clear_execution_state(&mut self) {
        // Reset contract:
        // - Clears register file, pc, frame/iterator stacks, saved execution stacks,
        //   try handlers, pending exceptions, and native call depth.
        // - Leaves kernel-owned shared state intact.
        // - `reset()` additionally clears bytecode/constants and resets epoch ownership.
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
        let (idx, hash) = self.kernel_core.string_forge().intern(s);
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
        let func_proto_ptr = self.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(func_proto_ptr);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
        obj.set_function(true);
        obj.set_sub_module_index(sub_idx);
        obj.set_class_constructor(is_class_constructor);
        obj.set_derived_constructor(is_derived_constructor);
        let _ = needs_home_object;
        if is_arrow {
            obj.set_arrow(true);
            obj.set_captured_this(self.regs[254]);
        }
        let obj_ptr = self.epoch.alloc(obj);
        let func_val = JsValue::object(obj_ptr as *mut u8);

        if !is_arrow {
            let object_proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
            let prototype_obj = self
                .epoch
                .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto_ptr)));
            let prototype_val = JsValue::from_js_object(prototype_obj);

            let constructor_si = self.kernel_core.string_forge().intern("constructor").0;
            let constructor_shape = self.kernel_core.shape_forge().make_shape(EMPTY_SHAPE_ID, constructor_si);
            let prototype = unsafe { &mut *prototype_obj };
            prototype.set_shape_id(constructor_shape);
            let constructor_pos = prototype.push_prop(func_val);
            prototype.set_data_meta(constructor_pos, PropAttributes::new(true, false, true));
            prototype.bump_generation();

            let prototype_si = self.kernel_core.string_forge().intern("prototype").0;
            let func = unsafe { &mut *obj_ptr };
            let prototype_shape = self.kernel_core.shape_forge().make_shape(func.shape_id(), prototype_si);
            func.set_shape_id(prototype_shape);
            func.ensure_hash_props().push(prototype_val);
            func.bump_generation();
        }

        func_val
    }

    pub(crate) fn error_text(&self, val: JsValue) -> String {
        if let Some(s) = self.lookup_str(val) {
            return s;
        }
        if val.is_object() {
            let obj = unsafe { &*val.as_js_object_ptr() };
            let name_si = self.kernel_core.string_forge().intern("name").0;
            let message_si = self.kernel_core.string_forge().intern("message").0;
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

    fn convert_constant(&mut self, constant: &Constant) -> Result<JsValue, JsError> {
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
                let pat_si = self.kernel_core.string_forge().intern(pattern).0;
                let flags_si = self.kernel_core.string_forge().intern(flags).0;
                let pat_val = JsValue::string(pat_si, 0);
                let flags_val = JsValue::string(flags_si, 0);

                let ctor_ptr = self.session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
                let ctor = unsafe { &*ctor_ptr };
                let Some(native_fn) = ctor.native_fn() else {
                    return Err(JsError::syntax_error("RegExp constructor unavailable"));
                };

                let saved_0 = self.regs[0];
                let saved_1 = self.regs[1];
                let saved_2 = self.regs[2];
                self.regs[0] = JsValue::undefined();
                self.regs[1] = pat_val;
                self.regs[2] = flags_val;
                let func = unsafe { native_fn_ptr_to_fn(native_fn) };
                let result = func(self, &[0, 1, 2]);
                self.regs[0] = saved_0;
                self.regs[1] = saved_1;
                self.regs[2] = saved_2;
                result.map_err(|err| JsError::syntax_error(self.error_text(err)))
            }
        }
    }

    pub(crate) fn convert_constants(&mut self, constants: &[Constant]) -> Result<Vec<JsValue>, JsError> {
        let mut values = Vec::with_capacity(constants.len());
        for constant in constants {
            values.push(self.convert_constant(constant)?);
        }
        Ok(values)
    }
}
