#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::bindings;
pub use crate::bindings::init_kernel_builtins;
use crate::coercion;
use crate::native::NativeFn;
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::prop_forge::PropTemplate;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

#[allow(unused_macros)]
macro_rules! throw_err {
    ($self:ident, Error, $msg:expr) => {{
        let error = crate::builtins::error::create_error($self, $msg);
        $self.exception_value = Some(error);
        $self.pending_error_kind = Some("Error");
        match $self.unwind() {
            Ok(()) => continue,
            Err(e) => return Err(e),
        }
    }};
    ($self:ident, TypeError, $msg:expr) => {{
        let error = crate::builtins::error::create_type_error($self, $msg);
        $self.exception_value = Some(error);
        $self.pending_error_kind = Some("TypeError");
        match $self.unwind() {
            Ok(()) => continue,
            Err(e) => return Err(e),
        }
    }};
    ($self:ident, ReferenceError, $msg:expr) => {{
        let error = crate::builtins::error::create_reference_error($self, $msg);
        $self.exception_value = Some(error);
        $self.pending_error_kind = Some("ReferenceError");
        match $self.unwind() {
            Ok(()) => continue,
            Err(e) => return Err(e),
        }
    }};
    ($self:ident, SyntaxError, $msg:expr) => {{
        let error = crate::builtins::error::create_syntax_error($self, $msg);
        $self.exception_value = Some(error);
        $self.pending_error_kind = Some("SyntaxError");
        match $self.unwind() {
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

macro_rules! set_or_create_prop {
    ($self:ident, $obj:expr, $prop_name_si:expr, $new_val:expr) => {{
        if let Some(pos) = $self
            .kernel
            .shape_forge()
            .lookup_position($obj.shape_id(), $prop_name_si)
        {
            $obj.set_prop_at(pos, $new_val);
        } else {
            let new_shape_id = $self
                .kernel
                .shape_forge()
                .make_shape($obj.shape_id(), $prop_name_si);
            $obj.set_shape_id(new_shape_id);
            $obj.push_prop($new_val);
            $obj.bump_generation();
        }
    }};
}

macro_rules! member_read_prop {
    ($self:ident, $obj:expr, $prop_name_si:expr) => {{
        let ext0 = $self.bytecode[$self.pc];
        let ext1 = $self.bytecode[$self.pc + 1];
        let ext2 = $self.bytecode[$self.pc + 2];
        $self.pc += 3;
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

        if cached_shape_id != 0 && cached_shape_id == $obj.shape_id() && cached_ptr != 0 {
            unsafe { *(cached_ptr as *const JsValue) }
        } else if let Some(template) = $self.kernel.prop_forge().get_template($obj.shape_id()) {
            if let Some(ptr) = $self.template_prop_ptr($obj, &template) {
                $self.bytecode[$self.pc - 3] = $obj.shape_id() & 0x00FF_FFFF;
                $self.bytecode[$self.pc - 2] = ptr as u32;
                $self.bytecode[$self.pc - 1] = (ptr as u64 >> 32) as u32;
                unsafe { *ptr }
            } else {
                $self
                    .resolve_property($obj, $prop_name_si)
                    .unwrap_or(JsValue::undefined())
            }
        } else if let Some(val) = $self.resolve_property($obj, $prop_name_si) {
            let pos = $self
                .kernel
                .shape_forge()
                .lookup_position($obj.shape_id(), $prop_name_si)
                .unwrap_or(0);
            if let Some(ptr) = $obj.prop_ptr_at(pos) {
                $self.bytecode[$self.pc - 3] = $obj.shape_id() & 0x00FF_FFFF;
                $self.bytecode[$self.pc - 2] = ptr as u32;
                $self.bytecode[$self.pc - 1] = (ptr as u64 >> 32) as u32;
            }
            val
        } else {
            JsValue::undefined()
        }
    }};
}

pub struct CallFrame {
    pub return_addr: usize,
    pub n_locals: u8,
    pub n_args: u8,
    pub function_obj_reg: u8,
    pub frame_base: u8,
    pub function_name: u32,
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
    pub for_in_iters: Vec<*mut u8>,
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
}

impl Vm {
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

    pub fn reset(&mut self) {
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode.clear();
        self.constants.clear();
        self.frames.clear();
        self.for_in_iters.clear();
        self.epoch.reset();
        self.interned_strings.clear();
    }

    pub fn intern(&mut self, s: &str) -> JsValue {
        let (idx, hash) = self.kernel.string_forge().intern(s);
        self.interned_strings.push(idx);
        JsValue::string(idx, hash)
    }

    /// Create a function JsObject for a BytecodeFunc constant.
    fn create_function_object(&mut self, sub_idx: u32) -> JsValue {
        let func_proto_ptr = self.kernel.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(func_proto_ptr);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
        obj.set_function(true);
        obj.set_sub_module_index(sub_idx);
        let obj_ptr = self.epoch.alloc(obj);
        JsValue::object(obj_ptr as *mut u8)
    }

    /// Convert a CompiledModule's constants into JsValue vec.
    fn convert_constants(&mut self, module: &CompiledModule) -> Vec<JsValue> {
        module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
                Constant::Int(v) => JsValue::int(*v),
                Constant::String(s) => self.intern(s),
                Constant::Boolean(b) => JsValue::bool(*b),
                Constant::Null => JsValue::null(),
                Constant::Undefined => JsValue::undefined(),
                Constant::BytecodeFunc(idx) => self.create_function_object(*idx),
                Constant::RegExp(pattern, flags) => {
                    let pat_si = self.kernel.string_forge().intern(pattern).0;
                    let flags_si = self.kernel.string_forge().intern(flags).0;
                    let pat_val = JsValue::string(pat_si, 0);
                    let flags_val = JsValue::string(flags_si, 0);

                    let native_fn = self.kernel.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
                    let ctor = unsafe { &*native_fn };
                    let ctor_fn = ctor.native_fn();
                    if ctor_fn.is_none() {
                        return JsValue::undefined();
                    }
                    let saved_0 = self.regs[0];
                    let saved_1 = self.regs[1];
                    let saved_2 = self.regs[2];
                    self.regs[0] = JsValue::undefined();
                    self.regs[1] = pat_val;
                    self.regs[2] = flags_val;
                    let func: crate::native::NativeFn = unsafe { std::mem::transmute(ctor_fn.unwrap()) };
                    let result = func(self, &[0, 1, 2]);
                    self.regs[0] = saved_0;
                    self.regs[1] = saved_1;
                    self.regs[2] = saved_2;
                    match result {
                        Ok(val) => val,
                        Err(_) => JsValue::undefined(),
                    }
                },
            })
            .collect()
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        self.kernel.string_forge().lookup(val.as_string_index())
    }

    fn template_prop_ptr(&self, obj: &JsObject, template: &PropTemplate) -> Option<*const JsValue> {
        let pos = template.position as usize;
        obj.hash_props_vec().and_then(|vec| {
            if pos < vec.len() {
                Some(&*vec[pos] as *const JsValue)
            } else {
                None
            }
        })
    }

    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self
                .kernel
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

    fn set_member_prop(
        &mut self,
        obj: &mut JsObject,
        prop_name_si: u32,
        val: JsValue,
    ) -> Result<(), String> {
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            obj.set_prop_at(pos, val);
            // IC write-back: 3 extension words (shape_id + ptr_lo + ptr_hi)
            if let Some(ptr) = obj.prop_ptr_at(pos) {
                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                self.bytecode[self.pc - 2] = ptr as u32;
                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
            }
        } else {
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            let new_pos = obj.push_prop(val);
            obj.bump_generation();
            if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                self.bytecode[self.pc - 2] = ptr as u32;
                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                self.kernel.prop_forge().upsert(
                    new_shape_id,
                    PropTemplate {
                        shape_id: new_shape_id,
                        position: new_pos,
                        generation: obj.generation(),
                    },
                );
            }
        }
        Ok(())
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.for_in_iters.clear();
        self.try_stack.clear();
        self.exception_value = None;
        self.pending_exception = None;
        self.pending_error_kind = None;
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
        self.constants = self.convert_constants(module);
        self.sub_modules = module.sub_modules.clone();
        self.sub_module_constants = vec![Vec::new(); self.sub_modules.len()];
        self.bytecode = module.bytecode.clone();
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.saved_bytecode_stack.clear();
        self.saved_constants_stack.clear();
        self.try_stack.clear();
        self.exception_value = None;
        self.pending_exception = None;
        self.pending_error_kind = None;

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            if let Some(pos) = self
                .kernel
                .shape_forge()
                .lookup_position(global.shape_id(), si)
            {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.dispatch()
    }

    /// Call a bytecode function from native code (D-09).
    /// Stub: sub_module storage not yet wired (plan 12.1-03).
    #[allow(dead_code)]
    pub fn call_bytecode_func(
        &mut self,
        _callback_obj: &JsObject,
        _args_regs: &[u8],
    ) -> Result<JsValue, String> {
        Err("bytecode function calls not yet supported".into())
    }

    pub(crate) fn unwind(&mut self) -> Result<(), String> {
        while let Some(handler) = self.try_stack.pop() {
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
        let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
        let kind_str = self.pending_error_kind.take().unwrap_or("Error");
        let msg = format!("uncaught {kind_str}: {exc}");
        Err(msg)
    }

    fn dispatch(&mut self) -> Result<JsValue, String> {
        let mut steps: u64 = 0;
        const MAX_STEPS: u64 = 100_000_000;
        loop {
            steps += 1;
            if steps > MAX_STEPS {
                return Err(format!("VM step limit exceeded at pc={}", self.pc));
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
                    let cond =
                        coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if !cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::JMP_IF_TRUE => {
                    let cond =
                        coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::LOAD_VAR => {
                    self.regs[rd] = self.regs[a];
                }

                OpCode::STORE_VAR => {
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
                                let ext = self.bytecode[self.pc];
                                self.pc += 1;
                                let arg_count = (ext & 0xFF) as usize;

                                if obj.native_fn().is_some() {
                                    match self.dispatch_native_call(
                                        obj,
                                        callee,
                                        this_reg,
                                        first_arg_reg,
                                        arg_count,
                                    ) {
                                        Ok(()) => continue,
                                        Err(e) => return Err(e),
                                    }
                                } else if obj.sub_module_index() > 0 {
                                    // 1-indexed sub_module_index (0 = not a bytecode function)
                                    let sub_idx = obj.sub_module_index() as usize - 1;
                                    if sub_idx >= self.sub_modules.len() {
                                        return Err(format!(
                                            "CALL: sub_module_index {} out of bounds (max {})",
                                            sub_idx,
                                            self.sub_modules.len()
                                        ));
                                    }

                                    if self.frames.len() >= self.kernel.config.max_call_depth {
                                        return Err(
                                            "RangeError: Maximum call stack size exceeded".into()
                                        );
                                    }

                                    // Clone sub_module data before mutably borrowing self
                                    let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
                                    let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
                                    let sub_constants = self.sub_modules[sub_idx].constants.clone();
                                    let sub_builtin_count =
                                        self.sub_modules[sub_idx].builtin_reg_map.len();

                                    // Copy args to regs[sub_builtin_count..sub_builtin_count+n_args]
                                    // (builtins occupy regs[0..sub_builtin_count-1])
                                    for i in 0..sub_n_args {
                                        let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                                        self.regs[sub_builtin_count + i] = self.regs[src_reg];
                                    }
                                    // Set this (reg 254 convention)
                                    self.regs[254] = callee;

                                    // Save current bytecode/constants
                                    self.saved_bytecode_stack
                                        .push(std::mem::take(&mut self.bytecode));
                                    self.saved_constants_stack
                                        .push(std::mem::take(&mut self.constants));

                                    // Push call frame
                                    self.frames.push(CallFrame {
                                        return_addr: self.pc,
                                        n_locals: sub_n_args as u8,
                                        n_args: sub_n_args as u8,
                                        function_obj_reg: callee_reg as u8,
                                        frame_base: sub_builtin_count as u8,
                                        function_name: 0,
                                    });

                                    // Convert sub_module constants
                                    self.bytecode = sub_bytecode;
                                    self.constants = sub_constants
                                        .iter()
                                        .map(|c| match c {
                                            Constant::Number(v) => JsValue::float(*v),
                                            Constant::Int(v) => JsValue::int(*v),
                                            Constant::String(s) => self.intern(s),
                                            Constant::Boolean(b) => JsValue::bool(*b),
                                            Constant::Null => JsValue::null(),
                                            Constant::Undefined => JsValue::undefined(),
                                            Constant::BytecodeFunc(idx) => {
                                                self.create_function_object(*idx)
                                            }
                                            Constant::RegExp(_, _) => JsValue::undefined(),
                                        })
                                        .collect();

                                    // Pre-fill builtin registers from the global object
                                    // (same as Vm::run does for the main module).
                                    for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                                        let si = self.kernel.string_forge().intern(name.as_str()).0;
                                        let global = self.kernel.global_object();
                                        if let Some(pos) = self
                                            .kernel
                                            .shape_forge()
                                            .lookup_position(global.shape_id(), si)
                                        {
                                            self.regs[*reg as usize] = global.get_prop_at(pos);
                                        }
                                    }

                                    self.pc = 0;
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
                        throw_err!(
                            self,
                            TypeError,
                            "CALL_NATIVE target is not a native function"
                        );
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
                        throw_err!(
                            self,
                            TypeError,
                            "NEW_EXPRESSION: constructor is not an object"
                        );
                    }
                    let ctor_ptr = constructor.as_js_object_ptr();
                    if ctor_ptr.is_null() {
                        throw_err!(self, TypeError, "NEW_EXPRESSION: constructor is null");
                    }
                    let ctor_obj = unsafe { &*ctor_ptr };
                    if !ctor_obj.is_function() {
                        throw_err!(
                            self,
                            TypeError,
                            "NEW_EXPRESSION: constructor is not a function"
                        );
                    }

                    // Read extension word for arg_count
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    // Create new empty object
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let new_obj = self.epoch.alloc(JsObject::new_empty(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                    ));

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

                        let func: NativeFn =
                            unsafe { std::mem::transmute(ctor_obj.native_fn().unwrap()) };
                        self.regs[254] = constructor;
                        match func(self, args_slice) {
                            Ok(val) => {
                                if val.is_object() {
                                    self.regs[rd] = val;
                                } else {
                                    self.regs[rd] = new_obj_val;
                                }
                            }
                            Err(err_val) => {
                                return Err(format!("Native constructor error: {:?}", err_val));
                            }
                        }
                    } else {
                        // Bytecode constructor - wired in plan 12.1-03
                        return Err(
                            "NEW_EXPRESSION: bytecode constructors not yet supported".into()
                        );
                    }
                }

                OpCode::RETURN => {
                    let result = self.regs[rd];
                    if let Some(frame) = self.frames.pop() {
                        // Restore caller's saved bytecode and constants
                        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
                            self.bytecode = saved_bc;
                        }
                        if let Some(saved_consts) = self.saved_constants_stack.pop() {
                            self.constants = saved_consts;
                        }
                        // Restore caller's pc and merge result into caller's regs[0].
                        self.pc = frame.return_addr;
                        self.regs[0] = result;
                    } else {
                        return Ok(result);
                    }
                }

                OpCode::IC_GET_PROP => {
                    let val = self.regs[a];
                    let obj_ptr = val.as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        if val.is_string() {
                            let proto_ptr =
                                self.kernel.builtin_world().string_proto.as_ptr() as *mut JsObject;
                            let proto = unsafe { &*proto_ptr };
                            let prop_name_si = self.regs[b].as_string_index();
                            if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                                self.regs[a] = resolved;
                                self.pc += 3;
                                continue;
                            }
                        }
                        if val.is_int() || val.is_double() {
                            let proto_ptr =
                                self.kernel.builtin_world().number_proto.as_ptr() as *mut JsObject;
                            let proto = unsafe { &*proto_ptr };
                            let prop_name_si = self.regs[b].as_string_index();
                            if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                                self.regs[a] = resolved;
                                self.pc += 3;
                                continue;
                            }
                        }
                        throw_err!(self, TypeError, "IC_GET_PROP on non-object");
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        self.regs[a] = unsafe { *(cached_ptr as *const JsValue) };
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let pos = template.position as usize;
                        if let Some(vec) = obj.hash_props_vec() {
                            if pos < vec.len() {
                                let ptr = &*vec[pos] as *const JsValue;
                                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                                self.regs[a] = unsafe { *(ptr) };
                            } else {
                                self.regs[a] = self
                                    .resolve_property(obj, prop_name_si)
                                    .unwrap_or(JsValue::undefined());
                            }
                        } else {
                            self.regs[a] = self
                                .resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined());
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                        self.regs[a] = val;
                    } else {
                        self.regs[a] = JsValue::undefined();
                    }
                }

                OpCode::IC_SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "IC_SET_PROP on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        unsafe {
                            *(cached_ptr as *mut JsValue) = self.regs[a];
                        }
                    } else {
                        if let Some(pos) = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                        {
                            obj.set_prop_at(pos, self.regs[a]);
                            if let Some(ptr) = obj.prop_ptr_at(pos) {
                                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            }
                        } else {
                            let new_shape_id = self
                                .kernel
                                .shape_forge()
                                .make_shape(obj.shape_id(), prop_name_si);
                            obj.set_shape_id(new_shape_id);
                            let new_pos = obj.push_prop(self.regs[a]);
                            obj.bump_generation();
                            if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                                self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                                self.kernel.prop_forge().upsert(
                                    new_shape_id,
                                    PropTemplate {
                                        shape_id: new_shape_id,
                                        position: new_pos,
                                        generation: obj.generation(),
                                    },
                                );
                            }
                        }
                    }
                }

                OpCode::GET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "GET_PROP on non-object");
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    self.regs[a] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }
                OpCode::SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "SET_PROP on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, self.regs[a]);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(self.regs[a]);
                        obj.bump_generation();
                    }
                }

                OpCode::GET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "GET_PROP_DYNAMIC on non-object");
                    }
                    let obj = unsafe { &*obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        throw_err!(self, TypeError, "GET_PROP_DYNAMIC key not a string");
                    }
                    let prop_name_si = key_val.as_string_index();
                    self.regs[b] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }

                OpCode::SET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "SET_PROP_DYNAMIC on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        throw_err!(self, TypeError, "SET_PROP_DYNAMIC key not a string");
                    }
                    let prop_name_si = key_val.as_string_index();
                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, self.regs[b]);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(self.regs[b]);
                        obj.bump_generation();
                    }
                }

                OpCode::NEW_OBJECT => {
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let obj = self.epoch.alloc(JsObject::new_empty(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                    ));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::NEW_ARRAY => {
                    let proto_ptr =
                        self.kernel.builtin_world().array_proto.as_ptr() as *mut JsObject;
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

                OpCode::SET_ELEM => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "SET_ELEM on non-object");
                    }
                    let idx = self.regs[a].as_int() as usize;
                    let obj = unsafe { &mut *obj_ptr };
                    obj.set_prop_at(idx as u8, self.regs[b]);
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

                OpCode::TYPEOF => {
                    self.dispatch_typeof(rd, a);
                }

                OpCode::VOID => {
                    self.regs[rd] = JsValue::undefined();
                }

                OpCode::IN => {
                    let key_val = self.regs[a];
                    let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "IN right-hand side is not an object");
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = if key_val.is_string() {
                        key_val.as_string_index()
                    } else {
                        throw_err!(self, TypeError, "IN key must be a string");
                    };
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

                OpCode::MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "MEMBER_INC on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        let new_pos = obj.push_prop(new_val);
                        obj.bump_generation();
                        if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                            self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    position: new_pos,
                                    generation: obj.generation(),
                                },
                            );
                        }
                        self.regs[a] = new_val;
                        continue;
                    };
                    self.regs[a] = new_val;
                }

                OpCode::MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "MEMBER_DEC on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        let new_pos = obj.push_prop(new_val);
                        obj.bump_generation();
                        if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                            self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    position: new_pos,
                                    generation: obj.generation(),
                                },
                            );
                        }
                        self.regs[a] = new_val;
                        continue;
                    };
                    self.regs[a] = new_val;
                }

                OpCode::DYN_MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "DYN_MEMBER_INC on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        throw_err!(self, TypeError, "DYN_MEMBER_INC key not a string");
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);
                    set_or_create_prop!(self, obj, prop_name_si, new_val);
                    self.regs[b] = new_val;
                }

                OpCode::DYN_MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "DYN_MEMBER_DEC on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        throw_err!(self, TypeError, "DYN_MEMBER_DEC key not a string");
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);
                    set_or_create_prop!(self, obj, prop_name_si, new_val);
                    self.regs[b] = new_val;
                }

                OpCode::COMPOUND_MEMBER_ADD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_ADD on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let rhs = self.regs[a];
                    let new_val = if prop_val.is_string() || rhs.is_string() {
                        let ls = coercion::to_string(self.kernel.string_forge().as_ref(), prop_val);
                        let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
                        let concat = format!("{ls}{rs}");
                        self.intern(&concat)
                    } else {
                        let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                        let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
                        JsValue::float(ln + rn)
                    };

                    self.set_member_prop(obj, prop_name_si, new_val)?;
                    self.regs[a] = new_val;
                }

                OpCode::COMPOUND_MEMBER_SUB => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_SUB on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln - rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_MUL => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_MUL on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln * rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_DIV => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_DIV on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln / rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_MOD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_MOD on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln % rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_EXP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        throw_err!(self, TypeError, "COMPOUND_MEMBER_EXP on non-object");
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let prop_val = member_read_prop!(self, obj, prop_name_si);

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln.powf(rn));
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
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
                                if shape.property_name != u32::MAX
                                    && seen.insert(shape.property_name)
                                {
                                    let hash = self
                                        .kernel
                                        .string_forge()
                                        .get_hash(shape.property_name)
                                        .unwrap_or(0);
                                    keys_vec.push(JsValue::string(shape.property_name, hash));
                                }
                                cursor = shape.parent;
                            } else {
                                break;
                            }
                        }
                        current = cur.proto();
                    }

                    let iter = self.epoch.alloc(ForInIter {
                        keys: keys_vec,
                        index: 0,
                    });
                    self.for_in_iters.push(iter as *mut u8);
                }

                OpCode::FOR_IN_NEXT => {
                    let iter_ptr = self
                        .for_in_iters
                        .last()
                        .copied()
                        .map(|p| p as *mut ForInIter)
                        .unwrap_or(std::ptr::null_mut());
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
                    let iter_ptr = self
                        .for_in_iters
                        .last()
                        .copied()
                        .map(|p| p as *mut ForInIter)
                        .unwrap_or(std::ptr::null_mut());
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

                OpCode::FOR_OF_INIT => {}

                OpCode::FOR_OF_NEXT => {},

                OpCode::FOR_OF_DONE => {},

                OpCode::FOR_OF_CLOSE => {},

                OpCode::THROW => {
                    let exc_value = self.regs[rd];
                    self.exception_value = Some(exc_value);
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
                    if !op.is_implemented() {
                        return Ok(JsValue::undefined());
                    }
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
