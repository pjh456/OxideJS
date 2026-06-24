use crate::coercion;
use crate::vm::Vm;
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
}
