use crate::coercion;
use crate::vm::Vm;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_eq(&mut self, rd: usize, a: usize, b: usize) {
        let eq = coercion::abstract_eq(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(eq);
    }

    #[inline(always)]
    pub(crate) fn dispatch_neq(&mut self, rd: usize, a: usize, b: usize) {
        let ne = !coercion::abstract_eq(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(ne);
    }

    #[inline(always)]
    pub(crate) fn dispatch_lt(&mut self, rd: usize, a: usize, b: usize) {
        let rel = coercion::relational_compare(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
    }

    #[inline(always)]
    pub(crate) fn dispatch_gt(&mut self, rd: usize, a: usize, b: usize) {
        let rel = coercion::relational_compare(self.regs[b], self.regs[a]);
        self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
    }

    #[inline(always)]
    pub(crate) fn dispatch_lte(&mut self, rd: usize, a: usize, b: usize) {
        let rel = coercion::relational_compare(self.regs[b], self.regs[a]);
        self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
    }

    #[inline(always)]
    pub(crate) fn dispatch_gte(&mut self, rd: usize, a: usize, b: usize) {
        let rel = coercion::relational_compare(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
    }

    #[inline(always)]
    pub(crate) fn dispatch_strict_eq(&mut self, rd: usize, a: usize, b: usize) {
        let eq = coercion::strict_equality(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(eq);
    }

    #[inline(always)]
    pub(crate) fn dispatch_strict_neq(&mut self, rd: usize, a: usize, b: usize) {
        let ne = !coercion::strict_equality(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(ne);
    }
}
