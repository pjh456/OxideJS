use crate::coercion;
use crate::vm::Vm;
use oxide_bytecode::opcode::{self, Instr};
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_jmp(&mut self, instr: Instr) {
        let offset = opcode::offset16(instr) as isize;
        self.pc = ((self.pc as isize) + offset - 1) as usize;
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_false(&mut self, rd: usize, instr: Instr) {
        let cond = coercion::to_boolean(self.regs[rd]);
        if !cond {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_true(&mut self, rd: usize, instr: Instr) {
        let cond = coercion::to_boolean(self.regs[rd]);
        if cond {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_nullish(&mut self, rd: usize, instr: Instr) {
        if self.regs[rd].is_nullish() {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_and(&mut self, rd: usize, a: usize, b: usize) {
        let val = self.regs[a];
        if coercion::to_boolean(val) {
            self.regs[rd] = self.regs[b];
        } else {
            self.regs[rd] = val;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_or(&mut self, rd: usize, a: usize, b: usize) {
        let val = self.regs[a];
        if coercion::to_boolean(val) {
            self.regs[rd] = val;
        } else {
            self.regs[rd] = self.regs[b];
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_nullish(&mut self, rd: usize, a: usize, b: usize) {
        let val = self.regs[a];
        self.regs[rd] = if val.is_nullish() { self.regs[b] } else { val };
    }

    #[inline(always)]
    pub(crate) fn dispatch_not(&mut self, rd: usize, a: usize) {
        let b = !coercion::to_boolean(self.regs[a]);
        self.regs[rd] = JsValue::bool(b);
    }
}
