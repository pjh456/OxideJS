use crate::coercion;
use crate::vm::Vm;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_and(&mut self, rd: usize, a: usize, b: usize) {
        let val = self.regs[a];
        if coercion::to_boolean(val, self.kernel_core.string_forge().as_ref()) {
            self.regs[rd] = self.regs[b];
        } else {
            self.regs[rd] = val;
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_or(&mut self, rd: usize, a: usize, b: usize) {
        let val = self.regs[a];
        if coercion::to_boolean(val, self.kernel_core.string_forge().as_ref()) {
            self.regs[rd] = val;
        } else {
            self.regs[rd] = self.regs[b];
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_not(&mut self, rd: usize, a: usize) {
        let b = !coercion::to_boolean(self.regs[a], self.kernel_core.string_forge().as_ref());
        self.regs[rd] = JsValue::bool(b);
    }
}
