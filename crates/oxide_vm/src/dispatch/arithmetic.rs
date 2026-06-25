use crate::vm::Vm;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_add(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let lhs = self.coerce_primitive_bounded(self.regs[a], false)?;
        let rhs = self.coerce_primitive_bounded(self.regs[b], false)?;
        if lhs.is_string() || rhs.is_string() {
            let ls = coercion::to_string(lhs);
            let rs = coercion::to_string(rhs);
            let concat = format!("{ls}{rs}");
            self.regs[rd] = self.new_string(&concat);
        } else {
            let ln = coercion::to_number(lhs);
            let rn = coercion::to_number(rhs);
            self.regs[rd] = JsValue::float(ln + rn);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_neg(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let v = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(-v);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_unary_plus(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let v = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(v);
        Ok(())
    }

    pub(crate) fn dispatch_compound_add(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let lhs = self.coerce_primitive_bounded(self.regs[rd], false)?;
        let rhs = self.coerce_primitive_bounded(self.regs[a], false)?;
        if lhs.is_string() || rhs.is_string() {
            let ls = coercion::to_string(lhs);
            let rs = coercion::to_string(rhs);
            let concat = format!("{ls}{rs}");
            self.regs[rd] = self.new_string(&concat);
        } else {
            let ln = coercion::to_number(lhs);
            let rn = coercion::to_number(rhs);
            self.regs[rd] = JsValue::float(ln + rn);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_sub(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let l = self.coerce_number_bounded(self.regs[rd])?;
        let r = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(l - r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_mul(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let l = self.coerce_number_bounded(self.regs[rd])?;
        let r = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(l * r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_div(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let l = self.coerce_number_bounded(self.regs[rd])?;
        let r = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(l / r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_mod(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let l = self.coerce_number_bounded(self.regs[rd])?;
        let r = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(l % r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_exp(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let l = self.coerce_number_bounded(self.regs[rd])?;
        let r = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(l.powf(r));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_inc_pre(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n + 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_inc_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n + 1.0);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_pre(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n - 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n - 1.0);
        Ok(())
    }
}
