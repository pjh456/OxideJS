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

/// int 自增：结果落在 i32 范围则保 int，否则升 double（与 int_add 同族语义，
/// 保持与 ADD 路径一致的 int 保持行为）。
#[inline(always)]
fn int_inc(a: i32) -> JsValue {
    match a.checked_add(1) {
        Some(v) => JsValue::int(v),
        None => JsValue::float(a as f64 + 1.0),
    }
}

/// int 自减：结果落在 i32 范围则保 int，否则升 double（与 int_sub 同族语义，
/// 保持与 SUB 路径一致的 int 保持行为）。
#[inline(always)]
fn int_dec(a: i32) -> JsValue {
    match a.checked_sub(1) {
        Some(v) => JsValue::int(v),
        None => JsValue::float(a as f64 - 1.0),
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

    /// 字符串拼接接线点（二元 `+`/`+=` 共用）：O(1) 链接为 Cons（rope）节点，
    /// 消除逐次整串重拷贝。单元序列在消费时（`.length`/`==`/单元读取等）惰性扁平化。
    ///
    /// # 步骤
    /// 1. 非字符串操作数先转成叶子字符串（`to_string` 同文本语义；小整数走
    ///    永久缓存零分配，其余 owned）。
    /// 2. 两个字符串经 `new_cons_string` 链接（每链接 1 次节点分配，对比原 O(n)
    ///    拷贝 + 结果串分配）。
    ///
    /// # 副作用
    /// - 新建 Cons 节点 + 可能的数字叶子，登记到 session 生命周期。回收点仅在
    ///   dispatch 指令边界（结果已写寄存器后检查），链接期子节点天然安全，
    ///   无需在途保护。
    fn concat_strings(&mut self, lhs: JsValue, rhs: JsValue) -> JsValue {
        // 小链急切扁平：总单元长 ≤ 阈值时单次预分配写齐，零叶子转换 / Cons 节点 /
        // 惰性扁平化开销——良形小串场景载荷与旧字节路径逐位一致（该场景 rope 的
        // O(1) 链接收益低于其固定开销）。
        let lu = if lhs.is_string() {
            unsafe { (*lhs.as_string_ptr()).utf16_len() as usize }
        } else {
            0
        };
        let ru = if rhs.is_string() {
            unsafe { (*rhs.as_string_ptr()).utf16_len() as usize }
        } else {
            0
        };
        if lu + ru <= Self::CONS_FLATTEN_UNITS {
            let mut units = Vec::with_capacity(lu + ru);
            coercion::push_units_to(lhs, &mut units);
            coercion::push_units_to(rhs, &mut units);
            return self.new_string_units_owned(units);
        }
        // 大链走 Cons rope：非字符串操作数先转叶子，再 O(1) 链接（单元惰性扁平化）。
        let lv = if lhs.is_string() { lhs } else { self.string_leaf(lhs) };
        let rv = if rhs.is_string() { rhs } else { self.string_leaf(rhs) };
        self.new_cons_string(lv, rv)
    }

    /// 把非字符串原语转为叶子字符串：0..=99 小整数命中永久缓存（零分配），
    /// 其余走 `to_string` + `new_string_owned`（语义与 `push_to_string` 一致）。
    fn string_leaf(&mut self, v: JsValue) -> JsValue {
        if v.is_int() {
            let n = v.as_int();
            if n >= 0 {
                if let Some(ptr) = oxide_kernel::string_forge::small_int_ptr(n as u32) {
                    return JsValue::string(ptr);
                }
            }
        }
        self.new_string_owned(coercion::to_string(v))
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
    /// 2. 精确总长：字符串操作数取单元长，非字符串给 32 单元余量（int/double/bool/null/
    ///    undefined 的十进制文本上界；BigInt 超长时 Vec 自动扩容兜底）。
    /// 3. 单趟单元展开 + 智能路由创建（零二次拷贝；良形内容落 Flat 与旧路径逐位一致）。
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
                unsafe { (*p.as_string_ptr()).utf16_len() as usize }
            } else {
                32
            }
        });
        let mut units = Vec::with_capacity(cap);
        for p in &parts {
            coercion::push_units_to(*p, &mut units);
        }
        Ok(self.new_string_units_owned(units))
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

    /// 幂运算核心：`base ** exponent` 的 Number/BigInt 双语义，二元 `**` 与复合
    /// `**=` 共用（避免两套实现漂移）。int/double 快路径与混合转数值路径都经
    /// IEEE-754 `powf`（`(-2) ** 0.5` → NaN、`0 ** 0` → 1、负底数整数指数等
    /// 边界由 libm pow 保证与规范一致）。
    ///
    /// # 边界与前提
    /// - BigInt 指数为负或超 u32：抛 RangeError（BigInt::exponentiate 禁止负指数，
    ///   超 u32 指数会分配不可控内存，同样拒绝）。
    /// - 混合 BigInt/Number → TypeError（ToNumeric 类型不一致）。
    /// - 对象操作数先 ToPrimitive（coerce 后判定，包装对象如 Object(2n) 参与 BigInt 运算）。
    ///
    /// # 副作用
    /// - 可能新建 BigInt 会话值（new_bigint）。
    fn exp_result(&mut self, lv: JsValue, rv: JsValue) -> Result<JsValue, String> {
        if lv.is_int() && rv.is_int() {
            return Ok(JsValue::float((lv.as_int() as f64).powf(rv.as_int() as f64)));
        }
        if lv.is_double() && rv.is_double() {
            return Ok(JsValue::float(lv.as_double().powf(rv.as_double())));
        }
        if lv.is_bigint() && rv.is_bigint() {
            let base = self.bigint_value(lv).clone();
            let exp = self.bigint_value(rv).clone();
            return self.bigint_exp(&base, &exp);
        }
        let l = self.coerce_primitive_bounded(lv, false)?;
        let r = self.coerce_primitive_bounded(rv, false)?;
        if l.is_bigint() && r.is_bigint() {
            let base = self.bigint_value(l).clone();
            let exp = self.bigint_value(r).clone();
            return self.bigint_exp(&base, &exp);
        }
        if l.is_bigint() != r.is_bigint() {
            // 包装对象 coerce 后暴露 BigInt：与另一非 BigInt 操作数混合必须抛
            // TypeError（同 ADD/SUB 抛点）。
            self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions")?;
            return Ok(JsValue::undefined());
        }
        let ln = coercion::to_number(l);
        let rn = coercion::to_number(r);
        Ok(JsValue::float(ln.powf(rn)))
    }

    /// BigInt 幂：负指数或超 u32 抛 RangeError，否则 `base.pow(exp)`。
    fn bigint_exp(&mut self, base: &num_bigint::BigInt, exp: &num_bigint::BigInt) -> Result<JsValue, String> {
        let exp_u32 = match u32::try_from(exp) {
            Ok(e) => e,
            Err(_) => {
                self.raise_error_kind("RangeError", "Exponent must be positive")?;
                return Ok(JsValue::undefined());
            }
        };
        Ok(self.new_bigint(base.pow(exp_u32)))
    }

    /// 二元幂运算：`regs[rd] = regs[a] ** regs[b]`。
    #[inline(always)]
    pub(crate) fn dispatch_exp(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("EXP rd={} r{}={:?} r{}={:?}", rd, a, self.regs[a], b, self.regs[b]);
        let lv = self.regs[a];
        let rv = self.regs[b];
        self.regs[rd] = self.exp_result(lv, rv)?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_compound_exp(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("COMPOUND_EXP rd={} r{}={:?}", rd, a, self.regs[a]);
        let lv = self.regs[rd];
        let rv = self.regs[a];
        self.regs[rd] = self.exp_result(lv, rv)?;
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
        if self.regs[rd].is_int() {
            let result = int_inc(self.regs[rd].as_int());
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
        if self.regs[rd].is_int() {
            let old = self.regs[rd];
            self.regs[a] = old;
            self.regs[rd] = int_inc(old.as_int());
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
        if self.regs[rd].is_int() {
            let result = int_dec(self.regs[rd].as_int());
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
        if self.regs[rd].is_int() {
            let old = self.regs[rd];
            self.regs[a] = old;
            self.regs[rd] = int_dec(old.as_int());
            return Ok(());
        }
        let n = self.coerce_number_bounded(self.regs[rd])?;
        self.regs[a] = JsValue::float(n);
        self.regs[rd] = JsValue::float(n - 1.0);
        Ok(())
    }
}
