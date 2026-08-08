use crate::vm::Vm;
use crate::vm_trace;
use oxide_bytecode::opcode::{self, Instr};
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_jmp(&mut self, instr: Instr) {
        let offset = opcode::offset16(instr) as isize;
        let target = ((self.pc as isize) + offset - 1) as usize;
        // try 体正常完成跳入 finally（`JMP finally_pc`）：标记 handler 进入 finally 体，
        // 供 break/continue/return/异常判定覆盖语义。
        if let Some(h) = self.try_stack.last_mut() {
            if h.frame_depth == self.frames.len() && h.finally_pc == Some(target) {
                h.finally_active = true;
            }
        }
        vm_trace!("JMP offset={}", offset);
        self.pc = target;
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_false(&mut self, rd: usize, instr: Instr) {
        vm_trace!("JMP_IF_FALSE r{}={:?}", rd, self.regs[rd]);
        let cond = coercion::to_boolean(self.regs[rd]);
        if !cond {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_true(&mut self, rd: usize, instr: Instr) {
        vm_trace!("JMP_IF_TRUE r{}={:?}", rd, self.regs[rd]);
        let cond = coercion::to_boolean(self.regs[rd]);
        if cond {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_jmp_if_nullish(&mut self, rd: usize, instr: Instr) {
        vm_trace!("JMP_IF_NULLISH r{}={:?}", rd, self.regs[rd]);
        if self.regs[rd].is_nullish() {
            let offset = opcode::offset16(instr) as isize;
            self.pc = ((self.pc as isize) + offset - 1) as usize;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_and(&mut self, rd: usize, a: usize, b: usize) {
        vm_trace!("LOGICAL_AND rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let val = self.regs[a];
        if coercion::to_boolean(val) {
            self.regs[rd] = self.regs[b];
        } else {
            self.regs[rd] = val;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_or(&mut self, rd: usize, a: usize, b: usize) {
        vm_trace!("LOGICAL_OR rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let val = self.regs[a];
        if coercion::to_boolean(val) {
            self.regs[rd] = val;
        } else {
            self.regs[rd] = self.regs[b];
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_nullish(&mut self, rd: usize, a: usize, b: usize) {
        vm_trace!("NULLISH rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let val = self.regs[a];
        self.regs[rd] = if val.is_nullish() { self.regs[b] } else { val };
    }

    #[inline(always)]
    pub(crate) fn dispatch_not(&mut self, rd: usize, a: usize) {
        vm_trace!("NOT rd={} r{}={:?}", rd, a, self.regs[a]);
        let b = !coercion::to_boolean(self.regs[a]);
        self.regs[rd] = JsValue::bool(b);
    }
}
