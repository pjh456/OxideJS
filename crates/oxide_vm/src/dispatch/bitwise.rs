use crate::coercion;
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
    pub(crate) fn dispatch_bit_and(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[a], sf) & coercion::to_int32(self.regs[b], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_or(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[a], sf) | coercion::to_int32(self.regs[b], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_xor(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[a], sf) ^ coercion::to_int32(self.regs[b], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_shl(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_int32(self.regs[a], sf);
        let shift = coercion::to_uint32(self.regs[b], sf) & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
    }

    #[inline(always)]
    pub(crate) fn dispatch_shr(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_int32(self.regs[a], sf);
        let shift = coercion::to_uint32(self.regs[b], sf) & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
    }

    #[inline(always)]
    pub(crate) fn dispatch_ushr(&mut self, rd: usize, a: usize, b: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_uint32(self.regs[a], sf);
        let shift = coercion::to_uint32(self.regs[b], sf) & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_not(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(!coercion::to_int32(self.regs[a], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_and(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[rd], sf) & coercion::to_int32(self.regs[a], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_or(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[rd], sf) | coercion::to_int32(self.regs[a], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_xor(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        self.regs[rd] = JsValue::int(coercion::to_int32(self.regs[rd], sf) ^ coercion::to_int32(self.regs[a], sf));
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shl(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_int32(self.regs[rd], sf);
        let shift = coercion::to_uint32(self.regs[a], sf) & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shr(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_int32(self.regs[rd], sf);
        let shift = coercion::to_uint32(self.regs[a], sf) & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_ushr(&mut self, rd: usize, a: usize) {
        let sf = self.kernel_core.string_forge().as_ref();
        let lhs = coercion::to_uint32(self.regs[rd], sf);
        let shift = coercion::to_uint32(self.regs[a], sf) & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
    }
}
