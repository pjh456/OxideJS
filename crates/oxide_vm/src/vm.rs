use std::collections::HashMap;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
use crate::mem::Epoch;
use crate::object::JsObject;
use crate::shape::EMPTY_SHAPE_ID;
use crate::value::JsValue;

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
    string_table: HashMap<String, u32>,
    string_reverse: Vec<String>,
    pub epoch: Epoch,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            string_table: HashMap::with_capacity(64),
            string_reverse: Vec::with_capacity(64),
            epoch: Epoch::new(),
        }
    }

    pub fn intern(&mut self, s: &str) -> JsValue {
        if let Some(&idx) = self.string_table.get(s) {
            return JsValue::string(idx, hash16(s));
        }
        let idx = self.string_reverse.len() as u32;
        self.string_reverse.push(s.to_string());
        self.string_table.insert(s.to_string(), idx);
        JsValue::string(idx, hash16(s))
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<&str> {
        if !val.is_string() {
            return None;
        }
        let idx = val.as_string_index() as usize;
        self.string_reverse.get(idx).map(|s| s.as_str())
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.string_table.clear();
        self.string_reverse.clear();

        let string_entries: Vec<String> = module
            .constants
            .iter()
            .filter_map(|c| {
                if let Constant::String(s) = c {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
        for s in &string_entries {
            self.intern(s);
        }
        self.constants = module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
                Constant::String(s) => {
                    let idx = self.string_table.get(s.as_str()).copied().unwrap_or(0);
                    JsValue::string(idx, hash16(s))
                }
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
                    let rel = coercion::relational_compare(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::GT => {
                    let rel = coercion::relational_compare(self.regs[b], self.regs[a]);
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::LTE => {
                    let rel = coercion::relational_compare(self.regs[b], self.regs[a]);
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::GTE => {
                    let rel = coercion::relational_compare(self.regs[a], self.regs[b]);
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

                OpCode::NEW_OBJECT => {
                    let obj = self
                        .epoch
                        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::NEW_ARRAY => {
                    let n = opcode::imm16(instr) as usize;
                    let obj =
                        self.epoch
                            .alloc(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), n));
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

fn hash16(s: &str) -> u16 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    s.hash(&mut h);
    (h.finish() >> 48) as u16
}
