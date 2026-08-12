use crate::vm::Vm;
use crate::vm_trace;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
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

    /// 位运算操作数公共路径：先 ToPrimitive（对象解盒，valueOf/toString 只触发一次），
    /// 返回原始值对；调用方再按 BigInt/Number 分流。混合 BigInt 与其它类型按规范抛 TypeError。
    #[inline(always)]
    fn coerce_primitive_pair(&mut self, lv: JsValue, rv: JsValue) -> Result<(JsValue, JsValue), String> {
        let saved_pc = self.pc;
        let l = self.coerce_primitive_bounded(lv, false)?;
        if self.pc != saved_pc {
            // 转换异常已被外围 try/catch 接住：unwind 已改写 pc，立即中止后续转换，
            // 由调用方在检测到 pc 变化后直接返回，避免继续用 undefined 计算结果并二次抛错。
            return Ok((JsValue::undefined(), JsValue::undefined()));
        }
        let r = self.coerce_primitive_bounded(rv, false)?;
        if self.pc != saved_pc {
            return Ok((JsValue::undefined(), JsValue::undefined()));
        }
        Ok((l, r))
    }

    /// BigInt 移位量转受控 i128：越界钳制到 ±2^24，防止极端移位 OOM；
    /// test262 BigInt 移位用例量级远小于该上限。
    fn bigint_shift_amount(r: &BigInt) -> i128 {
        const MAX_SHIFT: i128 = 1 << 24;
        let raw = r
            .to_i128()
            .unwrap_or_else(|| if r.sign() == Sign::Minus { i128::MIN } else { i128::MAX });
        raw.clamp(-MAX_SHIFT, MAX_SHIFT)
    }

    /// BigInt::leftShift(x, y)：y ≥ 0 左移 y 位；y < 0 右移 -y 位（floor，含负数）。
    fn bigint_left_shift(l: &BigInt, amount: i128) -> BigInt {
        if amount >= 0 {
            l << (amount as usize)
        } else {
            l >> ((-amount) as usize)
        }
    }

    /// BigInt::signedRightShift(x, y)：y ≥ 0 右移 y 位（floor，含负数）；y < 0 左移 -y 位。
    fn bigint_right_shift(l: &BigInt, amount: i128) -> BigInt {
        if amount >= 0 {
            l >> (amount as usize)
        } else {
            l << ((-amount) as usize)
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_and(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("BIT_AND rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) & self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? & self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_or(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("BIT_OR rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) | self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? | self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_xor(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("BIT_XOR rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) ^ self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? ^ self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_shl(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SHL rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            let amount = Self::bigint_shift_amount(self.bigint_value(r));
            self.regs[rd] = self.new_bigint(Self::bigint_left_shift(self.bigint_value(l), amount));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let lhs = self.coerce_int32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_shr(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SHR rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            let amount = Self::bigint_shift_amount(self.bigint_value(r));
            self.regs[rd] = self.new_bigint(Self::bigint_right_shift(self.bigint_value(l), amount));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let lhs = self.coerce_int32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_ushr(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("USHR rd={} r{},r{}", rd, a, b);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[a], self.regs[b])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() || r.is_bigint() {
            // 规范：BigInt 不支持无符号右移（Number/BigInt 混合与双 BigInt 均抛 TypeError）。
            return self.raise_type_error("BigInts have no unsigned right shift, use >> instead");
        }
        let lhs = self.coerce_uint32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_bit_not(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("BIT_NOT rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let v = self.coerce_primitive_bounded(self.regs[a], false)?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if v.is_bigint() {
            self.regs[rd] = self.new_bigint(!self.bigint_value(v));
            return Ok(());
        }
        self.regs[rd] = JsValue::int(!self.coerce_int32_bounded(v)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_and(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_BIT_AND rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) & self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? & self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_or(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_BIT_OR rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) | self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? | self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_bit_xor(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_BIT_XOR rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) ^ self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        self.regs[rd] = JsValue::int(self.coerce_int32_bounded(l)? ^ self.coerce_int32_bounded(r)?);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shl(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_SHL rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            let amount = Self::bigint_shift_amount(self.bigint_value(r));
            self.regs[rd] = self.new_bigint(Self::bigint_left_shift(self.bigint_value(l), amount));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let lhs = self.coerce_int32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.regs[rd] = JsValue::int(lhs.wrapping_shl(shift));
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_shr(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_SHR rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() && r.is_bigint() {
            let amount = Self::bigint_shift_amount(self.bigint_value(r));
            self.regs[rd] = self.new_bigint(Self::bigint_right_shift(self.bigint_value(l), amount));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let lhs = self.coerce_int32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.regs[rd] = JsValue::int(lhs >> shift);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_ushr(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_USHR rd={} r{}", rd, a);
        let saved_pc = self.pc;
        let (l, r) = self.coerce_primitive_pair(self.regs[rd], self.regs[a])?;
        if self.pc != saved_pc {
            return Ok(());
        }
        if l.is_bigint() || r.is_bigint() {
            return self.raise_type_error("BigInts have no unsigned right shift, use >> instead");
        }
        let lhs = self.coerce_uint32_bounded(l)?;
        let shift = self.coerce_uint32_bounded(r)? & 0x1F;
        self.write_ushr_result(rd, lhs >> shift);
        Ok(())
    }
}
