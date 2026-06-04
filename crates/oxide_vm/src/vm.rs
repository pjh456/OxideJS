use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
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
}

impl Vm {
    pub fn new() -> Self {
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
        }
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.constants = module.constants.iter().map(convert_constant).collect();
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
                    let lhs = coercion::to_primitive(self.regs[a]);
                    let rhs = coercion::to_primitive(self.regs[b]);
                    if lhs.is_object() || rhs.is_object() {
                        return Err("ADD with objects not yet supported".into());
                    }
                    if lhs.is_double()
                        && lhs.as_double().is_nan()
                        && lhs.as_double().to_bits() & 0x000F_FFFF_FFFF_FFFF == 0
                    {
                        // Actually this is too complex. Simpler: just check if it's a string via our JsValue type system.
                    }
                    // String concat: JsValue doesn't have is_string yet (Phase 6).
                    // For Phase 5, strings come as NaN-boxed objects — we don't have string JsValues.
                    // Addition defaults to numeric.
                    let ln = coercion::to_number(lhs);
                    let rn = coercion::to_number(rhs);
                    self.regs[rd] = JsValue::float(ln + rn);
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

fn convert_constant(c: &Constant) -> JsValue {
    match c {
        Constant::Number(v) => JsValue::float(*v),
        Constant::String(_s) => JsValue::null(),
        Constant::Boolean(b) => JsValue::bool(*b),
        Constant::Null => JsValue::null(),
        Constant::Undefined => JsValue::undefined(),
    }
}
