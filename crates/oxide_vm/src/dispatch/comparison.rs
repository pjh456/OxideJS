use crate::vm::Vm;
use crate::vm_trace;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_eq(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("EQ rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let lhs = self.regs[a];
        let rhs = self.regs[b];
        let eq = coercion::abstract_eq(lhs, rhs, self)?;
        self.regs[rd] = JsValue::bool(eq);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_neq(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("NEQ rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let lhs = self.regs[a];
        let rhs = self.regs[b];
        let ne = !coercion::abstract_eq(lhs, rhs, self)?;
        self.regs[rd] = JsValue::bool(ne);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_lt(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("LT rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let va = self.coerce_primitive_bounded(self.regs[a], false)?;
        let vb = self.coerce_primitive_bounded(self.regs[b], false)?;
        match coercion::relational_compare(va, vb) {
            Some(r) => self.regs[rd] = JsValue::bool(r),
            None => {
                vm_trace!("LT incomparable r{}={:?} r{}={:?}", a, self.regs[a], b, self.regs[b]);
                self.regs[rd] = JsValue::bool(false);
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_gt(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GT rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let va = self.coerce_primitive_bounded(self.regs[a], false)?;
        let vb = self.coerce_primitive_bounded(self.regs[b], false)?;
        match coercion::relational_compare(vb, va) {
            Some(r) => self.regs[rd] = JsValue::bool(r),
            None => {
                vm_trace!("GT incomparable r{}={:?} r{}={:?}", a, self.regs[a], b, self.regs[b]);
                self.regs[rd] = JsValue::bool(false);
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_lte(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("LTE rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let va = self.coerce_primitive_bounded(self.regs[a], false)?;
        let vb = self.coerce_primitive_bounded(self.regs[b], false)?;
        match coercion::relational_compare(vb, va) {
            Some(r) => self.regs[rd] = JsValue::bool(!r),
            None => {
                vm_trace!("LTE incomparable r{}={:?} r{}={:?}", a, self.regs[a], b, self.regs[b]);
                self.regs[rd] = JsValue::bool(false);
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_gte(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GTE rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let va = self.coerce_primitive_bounded(self.regs[a], false)?;
        let vb = self.coerce_primitive_bounded(self.regs[b], false)?;
        match coercion::relational_compare(va, vb) {
            Some(r) => self.regs[rd] = JsValue::bool(!r),
            None => {
                vm_trace!("GTE incomparable r{}={:?} r{}={:?}", a, self.regs[a], b, self.regs[b]);
                self.regs[rd] = JsValue::bool(false);
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_strict_eq(&mut self, rd: usize, a: usize, b: usize) {
        vm_trace!("STRICT_EQ rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let eq = coercion::strict_equality(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(eq);
    }

    #[inline(always)]
    pub(crate) fn dispatch_strict_neq(&mut self, rd: usize, a: usize, b: usize) {
        vm_trace!("STRICT_NEQ rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let ne = !coercion::strict_equality(self.regs[a], self.regs[b]);
        self.regs[rd] = JsValue::bool(ne);
    }
}
