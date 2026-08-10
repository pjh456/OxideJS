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
        if lv.is_bigint() && rv.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) + self.bigint_value(rv));
            return Ok(());
        }
        // 注意：不能在此前置检查 BigInt 混合（`lv.is_bigint() || rv.is_bigint()`）——
        // 对象操作数（如 `{valueOf: () => 2n}`）需先 ToPrimitive 再判定，混合检查
        // 必须放在 coerce 之后。
        let lhs = self.coerce_primitive_bounded(lv, false)?;
        let rhs = self.coerce_primitive_bounded(rv, false)?;
        if lhs.is_bigint() && rhs.is_bigint() {
            // 包装对象 coerce 后暴露双 BigInt（如 Object(2n) + 2n）。
            self.regs[rd] = self.new_bigint(self.bigint_value(lhs) + self.bigint_value(rhs));
            return Ok(());
        }
        if lhs.is_bigint() != rhs.is_bigint() {
            // 包装对象 coerce 后暴露 BigInt：与另一非 BigInt 操作数混合加法必须抛
            // TypeError（规范不允许 BigInt 与 Number/String 相加）。
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        if lhs.is_string() || rhs.is_string() {
            let lbytes = if lhs.is_string() { unsafe { (*lhs.as_string_ptr()).len() } } else { 0 };
            let rbytes = if rhs.is_string() { unsafe { (*rhs.as_string_ptr()).len() } } else { 0 };
            // 预分配精确容量：字符串操作数 O(1) 取字节长，一次分配写齐，免 push_str
            // 几何 realloc 的二次拷贝（字符串拼接热路径的主要额外成本）。
            // 32 字节余量覆盖数字/布尔等格式化文本（f64 文本最长约 24 字节），
            // 避免追加非字符串操作数时二次扩容。
            let mut buf = String::with_capacity(lbytes + rbytes + 32);
            coercion::push_to_string(lhs, &mut buf);
            coercion::push_to_string(rhs, &mut buf);
            let result = self.new_string_owned(buf);
            self.regs[rd] = result;
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
        let v = self.regs[a];
        if v.is_bigint() {
            self.regs[rd] = self.new_bigint(-self.bigint_value(v));
            return Ok(());
        }
        let v = self.coerce_number_bounded(v)?;
        self.regs[rd] = JsValue::float(-v);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_unary_plus(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("UNARY_PLUS rd={} r{}={:?}", rd, a, self.regs[a]);
        let v = self.regs[a];
        if v.is_bigint() {
            // 一元 + 对 BigInt 必须抛 TypeError（ToNumber(BigInt) 在隐式路径禁止）。
            return self.raise_type_error("Cannot convert a BigInt value to a number");
        }
        let v = self.coerce_number_bounded(v)?;
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
        if lv.is_bigint() && rv.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) + self.bigint_value(rv));
            return Ok(());
        }
        let lhs = self.coerce_primitive_bounded(lv, false)?;
        let rhs = self.coerce_primitive_bounded(rv, false)?;
        if lhs.is_bigint() && rhs.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lhs) + self.bigint_value(rhs));
            return Ok(());
        }
        if lhs.is_bigint() != rhs.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        if lhs.is_string() || rhs.is_string() {
            let lbytes = if lhs.is_string() { unsafe { (*lhs.as_string_ptr()).len() } } else { 0 };
            let rbytes = if rhs.is_string() { unsafe { (*rhs.as_string_ptr()).len() } } else { 0 };
            // 预分配精确容量：字符串操作数 O(1) 取字节长，一次分配写齐，免 push_str
            // 几何 realloc 的二次拷贝（字符串拼接热路径的主要额外成本）。
            // 32 字节余量覆盖数字/布尔等格式化文本（f64 文本最长约 24 字节），
            // 避免追加非字符串操作数时二次扩容。
            let mut buf = String::with_capacity(lbytes + rbytes + 32);
            coercion::push_to_string(lhs, &mut buf);
            coercion::push_to_string(rhs, &mut buf);
            let result = self.new_string_owned(buf);
            self.regs[rd] = result;
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
        if lv.is_bigint() && rv.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) - self.bigint_value(rv));
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) - self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(l);
        let rn = coercion::to_number(r);
        self.regs[rd] = JsValue::float(ln - rn);
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
        if lv.is_bigint() && rv.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) * self.bigint_value(rv));
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(l) * self.bigint_value(r));
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(l);
        let rn = coercion::to_number(r);
        self.regs[rd] = JsValue::float(ln * rn);
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
        if lv.is_bigint() && rv.is_bigint() {
            let r = self.bigint_value(rv);
            if r == 0 {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) / r);
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            let rv = self.bigint_value(r);
            if rv == 0 {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(l) / rv);
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(l);
        let rn = coercion::to_number(r);
        self.regs[rd] = JsValue::float(ln / rn);
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
        if lv.is_bigint() && rv.is_bigint() {
            let r = self.bigint_value(rv);
            if r == 0 {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) % r);
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            let rv = self.bigint_value(r);
            if rv == 0 {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(l) % rv);
            return Ok(());
        }
        if l.is_bigint() != r.is_bigint() {
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(l);
        let rn = coercion::to_number(r);
        self.regs[rd] = JsValue::float(ln % rn);
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
