use crate::vm::Vm;
use crate::vm_trace;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_add(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("ADD rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let lv = self.regs[a];
        let rv = self.regs[b];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 + rv.as_int() as f64);
            return Ok(());
        }
        let lhs = self.coerce_primitive_bounded(lv, false)?;
        let rhs = self.coerce_primitive_bounded(rv, false)?;
        if lhs.is_string() || rhs.is_string() {
            let mut buf = std::mem::take(&mut self.string_buf);
            buf.clear();
            coercion::push_to_string(lhs, &mut buf);
            coercion::push_to_string(rhs, &mut buf);
            self.regs[rd] = self.new_string(&buf);
            self.string_buf = buf;
        } else {
            let ln = coercion::to_number(lhs);
            let rn = coercion::to_number(rhs);
            self.regs[rd] = JsValue::float(ln + rn);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_neg(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("NEG rd={} r{}={:?}", rd, a, self.regs[a]);
        let v = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(-v);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_unary_plus(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("UNARY_PLUS rd={} r{}={:?}", rd, a, self.regs[a]);
        let v = self.coerce_number_bounded(self.regs[a])?;
        self.regs[rd] = JsValue::float(v);
        Ok(())
    }

    pub(crate) fn dispatch_compound_add(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_ADD rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 + rv.as_int() as f64);
            return Ok(());
        }
        let lhs = self.coerce_primitive_bounded(lv, false)?;
        let rhs = self.coerce_primitive_bounded(rv, false)?;
        if lhs.is_string() || rhs.is_string() {
            let mut buf = std::mem::take(&mut self.string_buf);
            buf.clear();
            coercion::push_to_string(lhs, &mut buf);
            coercion::push_to_string(rhs, &mut buf);
            self.regs[rd] = self.new_string(&buf);
            self.string_buf = buf;
        } else {
            let ln = coercion::to_number(lhs);
            let rn = coercion::to_number(rhs);
            self.regs[rd] = JsValue::float(ln + rn);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_sub(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_SUB rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 - rv.as_int() as f64);
            return Ok(());
        }
        let l = self.coerce_number_bounded(lv)?;
        let r = self.coerce_number_bounded(rv)?;
        self.regs[rd] = JsValue::float(l - r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_mul(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_MUL rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 * rv.as_int() as f64);
            return Ok(());
        }
        let l = self.coerce_number_bounded(lv)?;
        let r = self.coerce_number_bounded(rv)?;
        self.regs[rd] = JsValue::float(l * r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_div(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_DIV rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 / rv.as_int() as f64);
            return Ok(());
        }
        let l = self.coerce_number_bounded(lv)?;
        let r = self.coerce_number_bounded(rv)?;
        self.regs[rd] = JsValue::float(l / r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_mod(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_MOD rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float(lv.as_int() as f64 % rv.as_int() as f64);
            return Ok(());
        }
        let l = self.coerce_number_bounded(lv)?;
        let r = self.coerce_number_bounded(rv)?;
        self.regs[rd] = JsValue::float(l % r);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_exp(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_EXP rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = JsValue::float((lv.as_int() as f64).powf(rv.as_int() as f64));
            return Ok(());
        }
        let l = self.coerce_number_bounded(lv)?;
        let r = self.coerce_number_bounded(rv)?;
        self.regs[rd] = JsValue::float(l.powf(r));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_inc_pre(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("INC_PRE rd={} a={}", rd, a);
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n + 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_inc_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("INC_POST rd={} a={}", rd, a);
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n + 1.0);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_pre(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("DEC_PRE rd={} a={}", rd, a);
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n - 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("DEC_POST rd={} a={}", rd, a);
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n - 1.0);
        Ok(())
    }
}
