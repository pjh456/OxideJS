use crate::vm::Vm;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    fn write_ushr_result(&mut self, rd: usize, result: u32) {
        if result <= i32::MAX as u32 {
            self.regs[rd] = JsValue::int(result as i32);
        } else {
            self.regs[rd] = JsValue::float(result as f64);
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_and(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[a])? & self.coerce_int32_bounded(self.regs[b])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_or(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[a])? | self.coerce_int32_bounded(self.regs[b])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_xor(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[a])? ^ self.coerce_int32_bounded(self.regs[b])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_shl(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let lhs = self.coerce_int32_bounded(self.regs[a])?;
        let shift = self.coerce_uint32_bounded(self.regs[b])? & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_shr(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let lhs = self.coerce_int32_bounded(self.regs[a])?;
        let shift = self.coerce_uint32_bounded(self.regs[b])? & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_ushr(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let lhs = self.coerce_uint32_bounded(self.regs[a])?;
        let shift = self.coerce_uint32_bounded(self.regs[b])? & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_not(&mut self, rd: usize, a: usize) -> Result<(), String> {
        self.regs[rd] = JsValue::int(!self.coerce_int32_bounded(self.regs[a])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_and(&mut self, rd: usize, a: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[rd])? & self.coerce_int32_bounded(self.regs[a])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_or(&mut self, rd: usize, a: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[rd])? | self.coerce_int32_bounded(self.regs[a])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_xor(&mut self, rd: usize, a: usize) -> Result<(), String> {
        self.regs[rd] =
            JsValue::int(self.coerce_int32_bounded(self.regs[rd])? ^ self.coerce_int32_bounded(self.regs[a])?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shl(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let lhs = self.coerce_int32_bounded(self.regs[rd])?;
        let shift = self.coerce_uint32_bounded(self.regs[a])? & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shr(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let lhs = self.coerce_int32_bounded(self.regs[rd])?;
        let shift = self.coerce_uint32_bounded(self.regs[a])? & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_ushr(&mut self, rd: usize, a: usize) -> Result<(), String> {
        let lhs = self.coerce_uint32_bounded(self.regs[rd])?;
        let shift = self.coerce_uint32_bounded(self.regs[a])? & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
        Ok(())
    }
}
