#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
use crate::native::NativeFn;
use oxide_kernel::builtin::{ArrayMethods, ObjectMethods};
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::prop_forge::PropTemplate;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub struct CallFrame {
    pub return_addr: usize,
    pub n_locals: u8,
    pub n_args: u8,
}

pub struct ForInIter<'bump> {
    pub keys: bumpalo::collections::Vec<'bump, JsValue>,
    pub index: usize,
}

pub struct Vm {
    regs: [JsValue; 256],
    pc: usize,
    bytecode: Vec<opcode::Instr>,
    constants: Vec<JsValue>,
    frames: Vec<CallFrame>,
    pub for_in_iters: Vec<*mut u8>,
    kernel: Arc<OxideKernel>,
    interned_strings: Vec<u32>,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
}

pub fn init_kernel_builtins(kernel: &Arc<OxideKernel>) {
    let methods = ObjectMethods {
        keys: crate::builtins::object::object_keys as *const (),
        create: crate::builtins::object::object_create as *const (),
        assign: crate::builtins::object::object_assign as *const (),
        define_property: crate::builtins::object::object_define_property as *const (),
        get_own_property_descriptor: crate::builtins::object::object_get_own_property_descriptor
            as *const (),
    };
    kernel.builtin_world().bind_object_methods(
        &methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let global_ptr = kernel.global_object().as_ptr() as *mut JsObject;
    let global = unsafe { &mut *global_ptr };
    let si_obj = kernel.string_forge().intern("Object").0;
    let obj_shape = kernel.shape_forge().make_shape(global.shape_id(), si_obj);
    let obj_val = JsValue::from_js_object(
        kernel.builtin_world().object_constructor.as_ptr() as *mut JsObject
    );
    let cur_count = global.prop_count();
    global.set_shape_id(obj_shape);
    global.set_prop_count(cur_count + 1);
    let bump = bumpalo::Bump::new();
    global.set_prop_expand(cur_count, obj_val, &bump);

    let _array_methods = ArrayMethods {
        push: crate::builtins::array::array_push as *const (),
        pop: crate::builtins::array::array_pop as *const (),
        slice: crate::builtins::array::array_slice as *const (),
        splice: crate::builtins::array::array_splice as *const (),
        concat: crate::builtins::array::array_concat as *const (),
        join: crate::builtins::array::array_join as *const (),
        index_of: crate::builtins::array::array_index_of as *const (),
        includes: crate::builtins::array::array_includes as *const (),
        reverse: crate::builtins::array::array_reverse as *const (),
        for_each: crate::builtins::array::array_for_each as *const (),
        map: crate::builtins::array::array_map as *const (),
        filter: crate::builtins::array::array_filter as *const (),
        reduce: crate::builtins::array::array_reduce as *const (),
        find: crate::builtins::array::array_find as *const (),
        some: crate::builtins::array::array_some as *const (),
        every: crate::builtins::array::array_every as *const (),
        flat: crate::builtins::array::array_flat as *const (),
        flat_map: crate::builtins::array::array_flat_map as *const (),
    };

    kernel.builtin_world().bind_array_methods(
        &_array_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_arr = kernel.string_forge().intern("Array").0;
    let arr_shape = kernel.shape_forge().make_shape(global.shape_id(), si_arr);
    let arr_val =
        JsValue::from_js_object(kernel.builtin_world().array_constructor.as_ptr() as *mut JsObject);
    let cur_count = global.prop_count();
    global.set_shape_id(arr_shape);
    global.set_prop_count(cur_count + 1);
    let bump2 = bumpalo::Bump::new();
    global.set_prop_expand(cur_count, arr_val, &bump2);
    global.bump_generation();
}

impl Vm {
    pub fn new() -> Self {
        let kernel = Arc::new(OxideKernel::new(KernelConfig::minimal()));
        init_kernel_builtins(&kernel);
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
            object_prototype: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
        }
    }

    pub fn with_kernel(kernel: Arc<OxideKernel>) -> Self {
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            for_in_iters: Vec::new(),
            kernel: Arc::clone(&kernel),
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: P::clone(&kernel.builtin_world().object_proto),
        }
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

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        self.kernel.string_forge().lookup(val.as_string_index())
    }

    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        if let Some(offset) = self
            .kernel
            .shape_forge()
            .lookup_offset(obj.shape_id(), prop_name_si)
        {
            return Some(obj.get_prop(offset));
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(offset) = self
                .kernel
                .shape_forge()
                .lookup_offset(proto_obj.shape_id(), prop_name_si)
            {
                return Some(proto_obj.get_prop(offset));
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
        if let Some(offset) = self
            .kernel
            .shape_forge()
            .lookup_offset(obj.shape_id(), prop_name_si)
        {
            let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
            self.bytecode[self.pc - 1] = new_ext;
            obj.set_prop(offset, val);
        } else {
            let new_offset = obj.prop_count();
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
            self.bytecode[self.pc - 1] = new_ext;
            obj.set_shape_id(new_shape_id);
            obj.set_prop_count(new_offset + 1);
            obj.set_prop_expand(new_offset, val, self.epoch.bump());
            obj.bump_generation();
            self.kernel.prop_forge().upsert(
                new_shape_id,
                PropTemplate {
                    shape_id: new_shape_id,
                    offset: new_offset,
                    generation: obj.generation(),
                },
            );
        }
        Ok(())
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.for_in_iters.clear();
        self.dispatch()
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.constants = module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
                Constant::Int(v) => JsValue::int(*v),
                Constant::String(s) => self.intern(s),
                Constant::Boolean(b) => JsValue::bool(*b),
                Constant::Null => JsValue::null(),
                Constant::Undefined => JsValue::undefined(),
            })
            .collect();
        self.bytecode = module.bytecode.clone();
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            let mut shape_id = global.shape_id();
            let mut found: Option<u8> = None;
            while shape_id != EMPTY_SHAPE_ID {
                if let Some(shape) = self.kernel.shape_forge().get_shape(shape_id) {
                    if shape.property_name == si {
                        found = Some(shape.property_offset);
                        break;
                    }
                    shape_id = shape.parent.unwrap_or(EMPTY_SHAPE_ID);
                } else {
                    break;
                }
            }
            if let Some(offset) = found {
                self.regs[*reg as usize] = global.get_prop(offset);
            }
        }

        self.dispatch()
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
                    let idx = opcode::imm16(instr) as usize;
                    if idx < self.constants.len() {
                        self.regs[rd] = self.constants[idx];
                    } else {
                        return Err(format!("constant index {idx} out of bounds"));
                    }
                }

                OpCode::ADD => {
                    let lhs = self.regs[a];
                    let rhs = self.regs[b];
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

                OpCode::SUB => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l - r);
                }

                OpCode::MUL => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l * r);
                }

                OpCode::DIV => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l / r);
                }

                OpCode::MOD => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l % r);
                }

                OpCode::NEG => {
                    let v = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(-v);
                }

                OpCode::EQ => {
                    let eq = coercion::abstract_eq(
                        self.regs[a],
                        self.regs[b],
                        self.kernel.string_forge().as_ref(),
                    );
                    self.regs[rd] = JsValue::bool(eq);
                }

                OpCode::NEQ => {
                    let ne = !coercion::abstract_eq(
                        self.regs[a],
                        self.regs[b],
                        self.kernel.string_forge().as_ref(),
                    );
                    self.regs[rd] = JsValue::bool(ne);
                }

                OpCode::LT => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[a],
                        self.regs[b],
                    );
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::GT => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[b],
                        self.regs[a],
                    );
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::LTE => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[b],
                        self.regs[a],
                    );
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::GTE => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[a],
                        self.regs[b],
                    );
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::STRICT_EQ => {
                    let eq = coercion::strict_equality(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(eq);
                }

                OpCode::STRICT_NEQ => {
                    let ne = !coercion::strict_equality(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(ne);
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
                            if obj.is_function() && obj.native_fn().is_some() {
                                let ext = self.bytecode[self.pc];
                                self.pc += 1;
                                let arg_count = (ext & 0xFF) as usize;

                                let mut args_buf = [0u8; 257];
                                args_buf[0] = this_reg;
                                for i in 0..arg_count.min(256) {
                                    args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
                                }
                                let args_slice = &args_buf[..arg_count + 1];

                                let func: NativeFn =
                                    unsafe { std::mem::transmute(obj.native_fn().unwrap()) };
                                match func(self, args_slice) {
                                    Ok(val) => self.regs[0] = val,
                                    Err(err_val) => {
                                        return Err(format!("Native error: {:?}", err_val));
                                    }
                                }
                                continue;
                            }
                        }
                    }

                    return Err("CALL target is not a native function".into());
                }

                OpCode::RETURN => {
                    let result = self.regs[rd];
                    if let Some(frame) = self.frames.pop() {
                        self.pc = frame.return_addr;
                        self.regs[0] = result;
                    } else {
                        return Ok(result);
                    }
                }

                OpCode::IC_GET_PROP => {
                    let obj_ptr = self.regs[a].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("IC_GET_PROP on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        self.regs[a] = obj.get_prop(cached_offset);
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.regs[a] = obj.get_prop(template.offset);
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.regs[a] = val;
                    } else {
                        self.regs[a] = JsValue::undefined();
                    }
                }

                OpCode::IC_SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("IC_SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.set_prop(cached_offset, self.regs[a]);
                    } else {
                        if let Some(offset) = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                        {
                            let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
                            obj.set_prop(offset, self.regs[a]);
                        } else {
                            let new_offset = obj.prop_count();
                            let new_shape_id = self
                                .kernel
                                .shape_forge()
                                .make_shape(obj.shape_id(), prop_name_si);
                            obj.set_shape_id(new_shape_id);
                            obj.set_prop_count(new_offset + 1);
                            obj.set_prop(new_offset, self.regs[a]);
                            obj.bump_generation();
                            let new_ext =
                                (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    offset: new_offset,
                                    generation: obj.generation(),
                                },
                            );
                        }
                    }
                }

                OpCode::GET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("GET_PROP on non-object".into());
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
                        return Err("SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, self.regs[a]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, self.regs[a], self.epoch.bump());
                        obj.bump_generation();
                    }
                }

                OpCode::GET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("GET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("GET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    self.regs[b] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }

                OpCode::SET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("SET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("SET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, self.regs[b]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, self.regs[b], self.epoch.bump());
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
                        return Err("SET_ELEM on non-object".into());
                    }
                    let idx = self.regs[a].as_int() as usize;
                    let obj = unsafe { &mut *obj_ptr };
                    obj.set_prop(idx as u8, self.regs[b]);
                    if idx as u8 >= obj.prop_count() {
                        obj.set_prop_count(idx as u8 + 1);
                    }
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
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l - r);
                }

                OpCode::COMPOUND_MUL => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l * r);
                }

                OpCode::COMPOUND_DIV => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l / r);
                }

                OpCode::COMPOUND_MOD => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l % r);
                }

                OpCode::COMPOUND_EXP => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l.powf(r));
                }

                OpCode::TYPEOF => {
                    let val = self.regs[a];
                    let result = if val.is_undefined() {
                        "undefined"
                    } else if val.is_null() {
                        "object"
                    } else if val.is_bool() {
                        "boolean"
                    } else if val.is_int() || val.is_double() {
                        "number"
                    } else if val.is_string() {
                        "string"
                    } else if val.is_object() {
                        let obj = unsafe { &*val.as_js_object_ptr() };
                        if obj.is_function() {
                            "function"
                        } else {
                            "object"
                        }
                    } else {
                        "undefined"
                    };
                    self.regs[rd] = self.intern(result);
                }

                OpCode::VOID => {
                    self.regs[rd] = JsValue::undefined();
                }

                OpCode::IN => {
                    let key_val = self.regs[a];
                    let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("IN right-hand side is not an object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = if key_val.is_string() {
                        key_val.as_string_index()
                    } else {
                        return Err("IN key must be a string".into());
                    };
                    let found = self.resolve_property(obj, prop_name_si).is_some();
                    self.regs[rd] = JsValue::bool(found);
                }

                OpCode::NOT => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::bool(!cond);
                }

                OpCode::AND => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = if cond { self.regs[b] } else { self.regs[a] };
                }

                OpCode::OR => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = if cond { self.regs[a] } else { self.regs[b] };
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
                        return Err("MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    let offset = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        cached_offset
                    } else if let Some(off) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        off
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                        self.regs[a] = new_val;
                        continue;
                    };
                    obj.set_prop(offset, new_val);
                    self.regs[a] = new_val;
                }

                OpCode::MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    let offset = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        cached_offset
                    } else if let Some(off) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        off
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                        self.regs[a] = new_val;
                        continue;
                    };
                    obj.set_prop(offset, new_val);
                    self.regs[a] = new_val;
                }

                OpCode::DYN_MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("DYN_MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("DYN_MEMBER_INC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::DYN_MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("DYN_MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("DYN_MEMBER_DEC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::COMPOUND_MEMBER_ADD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_ADD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

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

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                    }
                    self.regs[a] = new_val;
                }

                OpCode::COMPOUND_MEMBER_SUB => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_SUB on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln - rn);
                    self.regs[a] = new_val;
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                    }
                }

                OpCode::COMPOUND_MEMBER_MUL => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_MUL on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln * rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_DIV => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_DIV on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln / rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_MOD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_MOD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln % rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_EXP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_EXP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln.powf(rn));
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::FOR_IN_INIT => {
                    let obj_val = self.regs[a];
                    if !obj_val.is_object() {
                        return Err("TypeError: for-in right-hand side is not an object".into());
                    }
                    let obj = unsafe { &*obj_val.as_js_object_ptr() };
                    let shape_id = obj.shape_id();

                    let mut keys_vec = bumpalo::collections::Vec::new_in(self.epoch.bump());
                    let mut cursor = Some(shape_id);
                    while let Some(id) = cursor {
                        if id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                            break;
                        }
                        if let Some(shape) = self.kernel.shape_forge().get_shape(id) {
                            if shape.property_name != u32::MAX {
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
