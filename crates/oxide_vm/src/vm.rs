#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::JsObject;
use oxide_types::shape::{self, EMPTY_SHAPE_ID};
use oxide_types::value::JsValue;

pub struct CallFrame {
    pub return_addr: usize,
    pub n_locals: u8,
    pub n_args: u8,
}

pub struct Vm {
    regs: [JsValue; 256],
    pc: usize,
    bytecode: Vec<opcode::Instr>,
    constants: Vec<JsValue>,
    frames: Vec<CallFrame>,
    kernel: Arc<OxideKernel>,
    interned_strings: Vec<u32>,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            kernel: Arc::new(OxideKernel::new(KernelConfig::minimal())),
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
        }
    }

    pub fn with_kernel(kernel: Arc<OxideKernel>) -> Self {
        let mut vm = Self::new();
        vm.kernel = Arc::clone(&kernel);
        vm.object_prototype = P::clone(&kernel.builtin_world().object_proto);
        vm
    }

    pub fn reset(&mut self) {
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode.clear();
        self.constants.clear();
        self.frames.clear();
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
        if let Some(offset) = shape::lookup_offset(obj.shape_id(), prop_name_si) {
            return Some(obj.get_prop(offset));
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(offset) = shape::lookup_offset(proto_obj.shape_id(), prop_name_si) {
                return Some(proto_obj.get_prop(offset));
            }
            proto = proto_obj.proto();
        }
        None
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.dispatch()
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.constants = module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
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

        self.dispatch()
    }

    fn dispatch(&mut self) -> Result<JsValue, String> {
        loop {
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
                        let ls = coercion::to_string(self, lhs);
                        let rs = coercion::to_string(self, rhs);
                        let concat = format!("{ls}{rs}");
                        self.regs[rd] = self.intern(&concat);
                    } else {
                        let ln = coercion::to_number(lhs);
                        let rn = coercion::to_number(rhs);
                        self.regs[rd] = JsValue::float(ln + rn);
                    }
                }

                OpCode::SUB => {
                    let l = coercion::to_number(self.regs[a]);
                    let r = coercion::to_number(self.regs[b]);
                    self.regs[rd] = JsValue::float(l - r);
                }

                OpCode::MUL => {
                    let l = coercion::to_number(self.regs[a]);
                    let r = coercion::to_number(self.regs[b]);
                    self.regs[rd] = JsValue::float(l * r);
                }

                OpCode::DIV => {
                    let l = coercion::to_number(self.regs[a]);
                    let r = coercion::to_number(self.regs[b]);
                    self.regs[rd] = JsValue::float(l / r);
                }

                OpCode::MOD => {
                    let l = coercion::to_number(self.regs[a]);
                    let r = coercion::to_number(self.regs[b]);
                    self.regs[rd] = JsValue::float(l % r);
                }

                OpCode::NEG => {
                    let v = coercion::to_number(self.regs[a]);
                    self.regs[rd] = JsValue::float(-v);
                }

                OpCode::EQ => {
                    let eq = coercion::abstract_eq(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(eq);
                }

                OpCode::NEQ => {
                    let ne = !coercion::abstract_eq(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(ne);
                }

                OpCode::LT => {
                    let rel = coercion::relational_compare(self, self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::GT => {
                    let rel = coercion::relational_compare(self, self.regs[b], self.regs[a]);
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::LTE => {
                    let rel = coercion::relational_compare(self, self.regs[b], self.regs[a]);
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::GTE => {
                    let rel = coercion::relational_compare(self, self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::JMP => {
                    let offset = opcode::offset16(instr) as isize;
                    self.pc = ((self.pc as isize) + offset - 1) as usize;
                }

                OpCode::JMP_IF_FALSE => {
                    let cond = coercion::to_boolean(self.regs[rd]);
                    if !cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::JMP_IF_TRUE => {
                    let cond = coercion::to_boolean(self.regs[rd]);
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
                    let offset = opcode::offset16(instr) as usize;
                    self.frames.push(CallFrame {
                        return_addr: self.pc,
                        n_locals: b as u8,
                        n_args: a as u8,
                    });
                    self.pc = offset;
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
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset =
                            shape::lookup_offset(obj.shape_id(), prop_name_si).unwrap_or(0);
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
                        if let Some(offset) = shape::lookup_offset(obj.shape_id(), prop_name_si) {
                            let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
                            obj.set_prop(offset, self.regs[a]);
                        } else {
                            let new_offset = obj.prop_count();
                            let new_shape_id = shape::make_shape(obj.shape_id(), prop_name_si);
                            obj.set_shape_id(new_shape_id);
                            obj.set_prop_count(new_offset + 1);
                            obj.set_prop(new_offset, self.regs[a]);
                            obj.bump_generation();
                            let new_ext =
                                (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
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
                    if let Some(offset) = shape::lookup_offset(obj.shape_id(), prop_name_si) {
                        obj.set_prop(offset, self.regs[a]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = shape::make_shape(obj.shape_id(), prop_name_si);
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
                    if let Some(offset) = shape::lookup_offset(obj.shape_id(), prop_name_si) {
                        obj.set_prop(offset, self.regs[b]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = shape::make_shape(obj.shape_id(), prop_name_si);
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
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let n = opcode::imm16(instr) as usize;
                    let obj = self.epoch.alloc(JsObject::new_array(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                        n,
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
