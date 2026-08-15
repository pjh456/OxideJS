use crate::vm::Vm;
use crate::vm_trace;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;
use smallvec::SmallVec;

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

    /// CONCAT_N：多操作数拼接（连续 `+` 左结合链摊平）的两阶段 dispatch。
    ///
    /// # 步骤
    /// 1. 读 ext 收集全部操作数寄存器（pc 推进约定同 TEMPLATE_STR：主循环已越过指令，
    ///    此处先读 ext[0]=n 并推进，再逐操作数推进，总推进 n 个扩展字）。
    /// 2. 阶段 1 数值前缀折叠：逐项复用 ADD 判定序列（int 溢出升 double / double /
    ///    BigInt 混合 TypeError / 对象 ToPrimitive 副作用顺序），与左结合两两 ADD
    ///    逐点一致；遇字符串即转入阶段 2。
    /// 3. 阶段 2 字符串模式单趟预分配：剩余操作数按序 coerce 后一次写齐，消除
    ///    N-2 次中间串分配。
    ///
    /// # 边界与前提
    /// - emit 层保证 n ≥ 3；此处 n<2 防御退化（返回首操作数）。
    /// - Symbol 操作数行为与 concat_strings 一致（push_to_string 无 symbol 分支）。
    ///
    /// # 副作用
    /// 写 regs[rd]；字符串路径新建一个会话字符串。
    #[inline(never)]
    pub(crate) fn dispatch_concat_n(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("CONCAT_N rd={}", rd);
        let n = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let mut ops = SmallVec::<[usize; 8]>::new();
        ops.push(a);
        for _ in 0..n.saturating_sub(1) {
            let reg = (self.bytecode[self.pc] & 0x7FFF_FFFF) as usize;
            self.pc += 1;
            ops.push(reg);
        }
        if ops.len() < 2 {
            self.regs[rd] = self.regs[ops[0]];
            return Ok(());
        }
        // ── 阶段 1：数值前缀折叠（与左结合两两 ADD 逐点一致）──
        let mut acc = self.regs[ops[0]];
        let mut idx = 1;
        while idx < ops.len() {
            let next = self.regs[ops[idx]];
            if acc.is_int() && next.is_int() {
                acc = int_add(acc.as_int(), next.as_int());
                idx += 1;
                continue;
            }
            if acc.is_double() && next.is_double() {
                acc = JsValue::float(acc.as_double() + next.as_double());
                idx += 1;
                continue;
            }
            if acc.is_bigint() && next.is_bigint() {
                acc = self.new_bigint(self.bigint_value(acc) + self.bigint_value(next));
                idx += 1;
                continue;
            }
            // 快路径失败才 coerce：acc 已原语零副作用；next 对象 ToPrimitive 在此触发，
            // 顺序与左结合 ADD 一致。
            let lhs = self.coerce_primitive_bounded(acc, false)?;
            let rhs = self.coerce_primitive_bounded(next, false)?;
            if lhs.is_string() || rhs.is_string() {
                // ── 阶段 2：字符串模式单趟预分配 ──
                self.regs[rd] = self.concat_n_strings(lhs, rhs, &ops[idx + 1..])?;
                return Ok(());
            }
            if lhs.is_bigint() && rhs.is_bigint() {
                acc = self.new_bigint(self.bigint_value(lhs) + self.bigint_value(rhs));
                idx += 1;
                continue;
            }
            if lhs.is_bigint() != rhs.is_bigint() {
                // 包装对象 coerce 后暴露 BigInt：混合加法必须抛 TypeError（同 ADD 抛点）。
                return self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions");
            }
            let ln = coercion::to_number(lhs);
            let rn = coercion::to_number(rhs);
            acc = JsValue::float(ln + rn);
            idx += 1;
        }
        self.regs[rd] = acc;
        Ok(())
    }

    /// 字符串模式单趟预分配拼接：l/r 已 coerce，剩余操作数按序 coerce 后一次写齐。
    ///
    /// # 步骤
    /// 1. 剩余操作数按序 coerce（对象 ToPrimitive 副作用顺序与左结合一致，抛错点先于分配）。
    /// 2. 精确总长：字符串操作数取字节长，非字符串给 32 字节余量（int/double/bool/null/
    ///    undefined 的十进制文本上界；BigInt 超长时 String 自动扩容兜底）。
    /// 3. 单趟 push_to_string + new_string_owned（零二次拷贝）。
    ///
    /// # 副作用
    /// 新建一个会话字符串。
    fn concat_n_strings(&mut self, lhs: JsValue, rhs: JsValue, rest: &[usize]) -> Result<JsValue, String> {
        let mut parts = SmallVec::<[JsValue; 8]>::new();
        parts.push(lhs);
        parts.push(rhs);
        for &reg in rest {
            let v = self.regs[reg];
            let prim = if v.is_object() { self.coerce_primitive_bounded(v, false)? } else { v };
            parts.push(prim);
        }
        let cap = parts.iter().fold(0usize, |acc, p| {
            acc + if p.is_string() {
                // SAFETY: p 是字符串值。
                unsafe { (*p.as_string_ptr()).len() }
            } else {
                32
            }
        });
        let mut buf = String::with_capacity(cap);
        for p in &parts {
            coercion::push_to_string(*p, &mut buf);
        }
        Ok(self.new_string_owned(buf))
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
                if let Some(v) = a.checked_rem(b) {
                    self.regs[rd] = JsValue::int(v);
                    return Ok(());
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
