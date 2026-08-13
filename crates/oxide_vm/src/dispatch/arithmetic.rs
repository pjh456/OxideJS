use crate::vm::Vm;
use crate::vm_trace;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

/// 判断 JsValue 是否为 BigInt 且值为 0（num_bigint 零值判断）。
fn bigint_is_zero(v: &num_bigint::BigInt) -> bool {
    num_traits::Zero::is_zero(v)
}

/// int+int 加法：结果落在 i32 范围则保 int，否则升 double。
#[inline(always)]
fn int_add(a: i32, b: i32) -> JsValue {
    match a.checked_add(b) {
        Some(v) => JsValue::int(v),
        None => JsValue::float(a as f64 + b as f64),
    }
}

/// int+int 减法：结果落在 i32 范围则保 int，否则升 double。
#[inline(always)]
fn int_sub(a: i32, b: i32) -> JsValue {
    match a.checked_sub(b) {
        Some(v) => JsValue::int(v),
        None => JsValue::float(a as f64 - b as f64),
    }
}

/// int+int 乘法：结果落在 i32 范围则保 int，否则升 double。
#[inline(always)]
fn int_mul(a: i32, b: i32) -> JsValue {
    match a.checked_mul(b) {
        Some(v) => JsValue::int(v),
        None => JsValue::float(a as f64 * b as f64),
    }
}

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_add(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("ADD rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let lv = self.regs[a];
        let rv = self.regs[b];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = int_add(lv.as_int(), rv.as_int());
            return Ok(());
        }
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() + rv.as_double());
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
        if lhs.is_string() || rhs.is_string() {
            self.regs[rd] = self.concat_strings(lhs, rhs);
            return Ok(());
        }
        if lhs.is_bigint() && rhs.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lhs) + self.bigint_value(rhs));
            return Ok(());
        }
        if lhs.is_bigint() != rhs.is_bigint() {
            // 包装对象 coerce 后暴露 BigInt：与另一非 BigInt 非字符串操作数混合加法
            // 必须抛 TypeError（规范只允许 String + BigInt 走字符串拼接）。
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(lhs);
        let rn = coercion::to_number(rhs);
        self.regs[rd] = JsValue::float(ln + rn);
        Ok(())
    }

    /// 字符串拼接热路径：一次预分配写齐两个操作数的 ToString 文本。
    ///
    /// # 步骤
    /// 1. 按字符串操作数字节长预分配容量（余量覆盖数字/布尔等格式化文本）。
    /// 2. 两个操作数依次走 push_to_string（BigInt 输出十进制，Symbol 由调用方先行拒绝）。
    /// 3. 生成会话字符串写入目标寄存器。
    ///
    /// # 副作用
    /// 新建一个会话字符串，登记到 session 生命周期。
    fn concat_strings(&mut self, lhs: JsValue, rhs: JsValue) -> JsValue {
        let lbytes = if lhs.is_string() { unsafe { (*lhs.as_string_ptr()).len() } } else { 0 };
        let rbytes = if rhs.is_string() { unsafe { (*rhs.as_string_ptr()).len() } } else { 0 };
        let mut buf = String::with_capacity(lbytes + rbytes + 32);
        coercion::push_to_string(lhs, &mut buf);
        coercion::push_to_string(rhs, &mut buf);
        self.new_string_owned(buf)
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
        // ToPrimitive 先解盒（BigInt 包装对象如 Object(1n) 的 valueOf 返回 BigInt），
        // 解盒后是 BigInt 才抛 TypeError——用户覆盖 valueOf/toString 的普通对象不受影响。
        let prim = self.coerce_primitive_bounded(v, false)?;
        if prim.is_bigint() {
            return self.raise_type_error("Cannot convert a BigInt value to a number");
        }
        let n = coercion::to_number(prim);
        self.regs[rd] = JsValue::float(n);
        Ok(())
    }

    pub(crate) fn dispatch_compound_add(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_ADD rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = int_add(lv.as_int(), rv.as_int());
            return Ok(());
        }
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() + rv.as_double());
            return Ok(());
        }
        if lv.is_bigint() && rv.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) + self.bigint_value(rv));
            return Ok(());
        }
        let lhs = self.coerce_primitive_bounded(lv, false)?;
        let rhs = self.coerce_primitive_bounded(rv, false)?;
        if lhs.is_string() || rhs.is_string() {
            self.regs[rd] = self.concat_strings(lhs, rhs);
            return Ok(());
        }
        if lhs.is_bigint() && rhs.is_bigint() {
            self.regs[rd] = self.new_bigint(self.bigint_value(lhs) + self.bigint_value(rhs));
            return Ok(());
        }
        if lhs.is_bigint() != rhs.is_bigint() {
            // 包装对象 coerce 后暴露 BigInt：与另一非 BigInt 非字符串操作数混合加法
            // 必须抛 TypeError（规范只允许 String + BigInt 走字符串拼接）。
            return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let ln = coercion::to_number(lhs);
        let rn = coercion::to_number(rhs);
        self.regs[rd] = JsValue::float(ln + rn);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_sub(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_SUB rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        if lv.is_int() && rv.is_int() {
            self.regs[rd] = int_sub(lv.as_int(), rv.as_int());
            return Ok(());
        }
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() - rv.as_double());
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
            self.regs[rd] = int_mul(lv.as_int(), rv.as_int());
            return Ok(());
        }
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() * rv.as_double());
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
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() / rv.as_double());
            return Ok(());
        }
        if lv.is_bigint() && rv.is_bigint() {
            let r = self.bigint_value(rv);
            if bigint_is_zero(r) {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) / r);
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            let rv = self.bigint_value(r);
            if bigint_is_zero(rv) {
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
            let a = lv.as_int();
            let b = rv.as_int();
            if b != 0 {
                match a.checked_rem(b) {
                    Some(v) => {
                        self.regs[rd] = JsValue::int(v);
                        return Ok(());
                    }
                    None => {}
                }
            }
            self.regs[rd] = JsValue::float(a as f64 % b as f64);
            return Ok(());
        }
        if lv.is_double() && rv.is_double() {
            self.regs[rd] = JsValue::float(lv.as_double() % rv.as_double());
            return Ok(());
        }
        if lv.is_bigint() && rv.is_bigint() {
            let r = self.bigint_value(rv);
            if bigint_is_zero(r) {
                return self.raise_error_kind("RangeError", "Division by zero");
            }
            self.regs[rd] = self.new_bigint(self.bigint_value(lv) % r);
            return Ok(());
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            let rv = self.bigint_value(r);
            if bigint_is_zero(rv) {
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
        if self.regs[rd].is_bigint() {
            let v = self.bigint_value(self.regs[rd]) + 1;
            let result = self.new_bigint(v);
            self.regs[rd] = result;
            self.regs[a] = result;
            return Ok(());
        }
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n + 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_inc_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("INC_POST rd={} a={}", rd, a);
        if self.regs[rd].is_bigint() {
            let v = self.bigint_value(self.regs[rd]).clone();
            self.regs[a] = self.regs[rd];
            self.regs[rd] = self.new_bigint(v + 1);
            return Ok(());
        }
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n + 1.0);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_pre(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("DEC_PRE rd={} a={}", rd, a);
        if self.regs[rd].is_bigint() {
            let v = self.bigint_value(self.regs[rd]).clone() - 1;
            let result = self.new_bigint(v);
            self.regs[rd] = result;
            self.regs[a] = result;
            return Ok(());
        }
        let n = self.coerce_number_bounded(self.regs[rd])?;
        let result = JsValue::float(n - 1.0);
        self.regs[rd] = result;
        self.regs[a] = result;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_dec_post(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("DEC_POST rd={} a={}", rd, a);
        if self.regs[rd].is_bigint() {
            let v = self.bigint_value(self.regs[rd]).clone();
            self.regs[a] = self.regs[rd];
            self.regs[rd] = self.new_bigint(v - 1);
            return Ok(());
        }
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n - 1.0);
        Ok(())
    }
}
