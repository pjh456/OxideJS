//! `oxide_runtime_api` —— builtins 与 VM 之间的抽象接口。
//!
//! Builtins 以泛型方式针对 [`VmHost`] trait 编写
//! （`fn xxx<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult`）；`Vm`
//! 实现 `VmHost`。这打破了 builtins crate 与 `oxide_vm` 之间本会形成的
//! 循环依赖：
//!
//! `oxide_types ← oxide_kernel ← oxide_runtime_api ← oxide_builtins ← oxide_vm`
//!
//! trait 面向泛型而非对象安全：单态化 `H = Vm` 使每个 `host.*()` 调用内联，
//! 相对 builtins 直接位于 `oxide_vm` 内部没有运行时开销。

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::JsValue;

/// 每个 builtin native 函数的返回值。
pub enum NativeResult {
    Ok(JsValue),
    Err(JsValue),
    TailCall { callee: JsValue, this: JsValue, args: Vec<JsValue> },
}

impl NativeResult {
    /// 构造成功结果，携带一个 `JsValue` 返回值。
    pub fn ok(val: JsValue) -> Self {
        Self::Ok(val)
    }

    /// 构造失败结果，携带被抛出的异常值。
    pub fn err(val: JsValue) -> Self {
        Self::Err(val)
    }

    /// 取出成功值；若为 `Err` 或 `TailCall` 则 panic。仅用于已知必然成功的场景。
    pub fn unwrap(self) -> JsValue {
        match self {
            Self::Ok(val) => val,
            Self::Err(_) => panic!("called `NativeResult::unwrap()` on an `Err` value"),
            Self::TailCall { .. } => panic!("called `NativeResult::unwrap()` on a `TailCall` value"),
        }
    }

    /// 把 `Err` 分支的错误值映射为自定义错误类型并转为 `Result`；
    /// `TailCall` 不能转换，遇到时 panic。
    pub fn map_err<E, F>(self, op: F) -> Result<JsValue, E>
    where
        F: FnOnce(JsValue) -> E,
    {
        match self {
            Self::Ok(val) => Ok(val),
            Self::Err(err) => Err(op(err)),
            Self::TailCall { .. } => panic!("TailCall cannot be converted to Result"),
        }
    }
}

/// builtins 依赖的 `Vm` 能力集合。
///
/// 方法签名是 `Vm` 上同名固有方法的逐字节拷贝；`impl VmHost for Vm` 委托给
/// 它们。trait 刻意保持扁平且非对象安全——builtins 始终接受 `&mut impl VmHost`。
pub trait VmHost {
    // 寄存器访问
    fn reg(&self, idx: u8) -> JsValue;
    fn set_reg(&mut self, idx: u8, val: JsValue);

    // 对象分配 / 字符串创建
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject;
    fn new_string(&mut self, s: &str) -> JsValue;

    // 内核访问器
    fn kernel_core(&self) -> &Arc<KernelCore>;
    fn session(&self) -> &KernelSession;
    fn epoch(&self) -> &Epoch;

    // 属性解析
    fn property_key_si(&mut self, val: JsValue) -> u32;
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue>;
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32>;

    // 属性访问
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String>;
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String>;

    // 属性定义
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue);

    // 查找 / 强制转换
    fn lookup_str(&self, val: JsValue) -> Option<String>;
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String>;
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String>;

    // 调用基础设施
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String>;
    /// 取回在 String 展平调用边界上保留下来的原始抛出 JsValue，
    /// 使迭代器包装器能重新抛出原错误而非二次包装。
    fn take_uncaught_value(&mut self) -> Option<JsValue>;

    // 错误处理
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String>;
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String>;
    fn error_message_text(&self, kind: &str, msg: &str) -> String;
    fn call_stack_function_names(&self) -> Vec<String>;
    fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue;
    fn step_rng(&mut self);
    fn math_rng_value(&self) -> f64;
    fn sub_module_function_name(&self, sub_idx: u16) -> String;
    /// 动态编译一个函数体（`Function` 构造器用）：把参数列表与函数体编译为可调用
    /// 函数对象。编译或解析失败返回 `Err`，由调用方转为 `SyntaxError`。
    fn create_dynamic_function(&mut self, params: &[String], body: &str) -> Result<JsValue, String>;
    fn symbol_intern(&mut self, desc: String) -> u32;
    fn symbol_description(&self, idx: u32) -> Option<&str>;
    fn symbol_lookup_global(&self, key: &str) -> Option<u32>;
    fn symbol_register_global(&mut self, key: String, idx: u32);
    fn symbol_key_for_id(&self, idx: u32) -> Option<String>;
}

/// 拼接 JS 错误消息，格式为 `"{name}: {msg}"`；任一段为空时只取非空段。
pub fn format_error_message(name: &str, msg: &str) -> String {
    if name.is_empty() {
        msg.to_string()
    } else if msg.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {msg}")
    }
}

/// 借出字符串值的文本内容。
///
/// # Safety
/// `val` 必须是字符串 `JsValue`，且其 `JsString` 指针存活。
#[inline]
pub unsafe fn string_data(val: JsValue) -> &'static str {
    (*val.as_string_ptr()).as_str()
}

/// 按内容比较两个字符串 `JsValue` 是否相等。
///
/// 先做指针级短路（同一 interned 字符串必等），否则逐字节比较 `JsString` 内容。
#[inline]
pub fn string_value_eq(a: JsValue, b: JsValue) -> bool {
    if a.as_string_ptr() == b.as_string_ptr() {
        return true;
    }
    let sa = unsafe { &*a.as_string_ptr() };
    let sb = unsafe { &*b.as_string_ptr() };
    sa.data == sb.data
}

/// 对原始值执行 ECMAScript ToNumber 的快速路径（不触发对象 coercion）。
///
/// Number/String/Boolean/null 按规范转换；undefined 与不可解析字符串为 `NaN`；
/// Object 与 Symbol 不在此处理，返回 `NaN`（对象需走 [`to_number_full`]）。
pub fn to_number(val: JsValue) -> f64 {
    if val.is_int() {
        return val.as_int() as f64;
    }
    if val.is_double() {
        return val.as_double();
    }
    if val.is_bool() {
        return if val.as_bool() { 1.0 } else { 0.0 };
    }
    if val.is_null() {
        return 0.0;
    }
    if val.is_undefined() {
        return f64::NAN;
    }
    if val.is_string() {
        let s = unsafe { string_data(val) };
        return parse_js_number(s);
    }
    if val.is_object() {
        return f64::NAN;
    }
    f64::NAN
}

/// 按 ECMA-262 ToNumber 的 StringNumericLiteral 语法解析字符串。
///
/// 在 Rust `parse::<f64>` 之上补齐 JS 特有规则：空串为 0、`0x/0o/0b` 前缀按
/// 对应进制解析、精确匹配 `±Infinity`。Rust parse 对 inf/nan 大小写不敏感，
/// 需先按十进制字符集排除这些令牌（JS 只接受精确的 "Infinity"）。
///
/// # 边界与前提
/// - trim 后空串 → 0
/// - 十六/八/二进制要求全部字符为有效数字，含非法字符 → NaN
/// - 十进制结果与 Rust parse 一致，溢出时归 ±inf / 0
fn parse_js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix("0x") {
        return parse_radix_int(digits, 16);
    }
    if let Some(digits) = lower.strip_prefix("0o") {
        return parse_radix_int(digits, 8);
    }
    if let Some(digits) = lower.strip_prefix("0b") {
        return parse_radix_int(digits, 2);
    }
    if t == "Infinity" || t == "+Infinity" {
        return f64::INFINITY;
    }
    if t == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    // 十进制语法仅允许数字、符号、小数点与指数 e；含其它字符的令牌（如
    // inf/nan 变体）不是合法 StringNumericLiteral，一律 NaN。
    if !t
        .chars()
        .all(|c| c.is_ascii_digit() || c == '+' || c == '-' || c == '.' || c == 'e' || c == 'E')
    {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// 解析指定进制的无符号整数；全部字符必须是有效数字，否则返回 NaN。
///
/// 累积在 f64 中：2/8/16 是 2 的幂，逐位乘加即为正确舍入结果；十进制不适用。
fn parse_radix_int(s: &str, radix: u32) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    let mut v = 0.0f64;
    for c in s.chars() {
        match c.to_digit(radix) {
            Some(d) => v = v * radix as f64 + d as f64,
            None => return f64::NAN,
        }
    }
    v
}

/// 把 f64 格式化为 ECMA-262 Number::toString 的字符串（含 NaN/±Infinity 专名）。
///
/// 对有限非零数，先经 ryu 最短表示取出有效数字与十进制指数（`format_finite`
/// 输出形如 `"123.45"` / `"1e21"` / `"0.001"`），再按规范分段重建：指数
/// `-6 < n <= 21` 时用定点表示，其余用科学计数法 `d.ddde±e`。替代原先
/// `(d as i64)` 直接转换——后者对 `>= 2^63` 的数值溢出。
///
/// # 步骤
/// 1. 取绝对值经 ryu `format_finite` 得最短表示 `s`
/// 2. 含 `e` 时解析出有效数字与指数，按四段规则重建（整数/带小数点/纯小数/科学计数）
/// 3. 不含 `e` 的定点形式仅需去掉整数末尾的 `.0` 后缀
///
/// # 边界与前提
/// - NaN → "NaN"，±∞ → "±Infinity"，±0 → "0"
/// - 负数先格式化绝对值再加 "-" 前缀
pub fn js_number_to_string(d: f64) -> String {
    if d.is_nan() {
        return "NaN".to_string();
    }
    if d.is_infinite() {
        return if d.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    if d == 0.0 {
        return "0".to_string();
    }
    let neg = d.is_sign_negative();
    let mut buf = ryu::Buffer::new();
    let s = buf.format_finite(d.abs());
    let mut out = String::new();
    if let Some(e_pos) = s.find('e') {
        let digits: Vec<char> = s[..e_pos].chars().filter(|c| *c != '.').collect();
        let exp: i32 = s[e_pos + 1..].parse().unwrap_or(0);
        let k = digits.len() as i32;
        let n = exp + 1;
        if n > -6 && n <= 21 {
            // 定点：ryu 在此范围输出指数形式但 JS 要求十进制。
            if n <= 0 {
                out.push_str("0.");
                for _ in 0..(-n) {
                    out.push('0');
                }
                out.extend(digits.iter());
            } else if k <= n {
                out.extend(digits.iter());
                for _ in 0..(n - k) {
                    out.push('0');
                }
            } else {
                out.extend(digits[..n as usize].iter());
                out.push('.');
                out.extend(digits[n as usize..].iter());
            }
        } else {
            // 科学计数：首位 + (可选 "." + 剩余) + e[+/-]指数。
            out.push(digits[0]);
            if k > 1 {
                out.push('.');
                out.extend(digits[1..].iter());
            }
            out.push('e');
            if n > 1 {
                out.push('+');
            }
            out.push_str(&(n - 1).to_string());
        }
    } else if let Some(stripped) = s.strip_suffix(".0") {
        out.push_str(stripped);
    } else {
        out.push_str(s);
    }
    if neg {
        let mut res = String::with_capacity(out.len() + 1);
        res.push('-');
        res.push_str(&out);
        res
    } else {
        out
    }
}

/// ToUint32（ECMA-262 §7.1.6）：对数值取模 2^32。NaN/±0/Infinity 归零。
pub fn to_uint32(val: JsValue) -> u32 {
    let n = to_number(val);
    if n == 0.0 || !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// ToInt32（ECMA-262 §7.1.5）：对数值取模 2^32 后按有符号 32 位解释。
pub fn to_int32(val: JsValue) -> i32 {
    let int = to_uint32(val);
    if int > i32::MAX as u32 {
        (int as i64 - 4_294_967_296i64) as i32
    } else {
        int as i32
    }
}

/// 把 `val` 的字符串表示追加到 `buf`，不分配中间临时 String。
/// 用于热路径字符串拼接。
pub fn push_to_string(val: JsValue, buf: &mut String) {
    if val.is_int() {
        use std::fmt::Write;
        let _ = write!(buf, "{}", val.as_int());
        return;
    }
    if val.is_double() {
        buf.push_str(&js_number_to_string(val.as_double()));
        return;
    }
    if val.is_bool() {
        buf.push_str(if val.as_bool() { "true" } else { "false" });
        return;
    }
    if val.is_null() {
        buf.push_str("null");
        return;
    }
    if val.is_undefined() {
        buf.push_str("undefined");
        return;
    }
    if val.is_string() {
        unsafe { buf.push_str(string_data(val)) };
        return;
    }
    if val.is_object() {
        buf.push_str("[object]");
    }
}

/// 把原始值转成字符串（ToString 的原始值路径）。
///
/// 数值走 ECMA-262 Number::toString 格式化；Object 在此返回占位符 `[object]`，
/// 完整路径见 [`to_string_full`]。
pub fn to_string(val: JsValue) -> String {
    if val.is_int() {
        return val.as_int().to_string();
    }
    if val.is_double() {
        return js_number_to_string(val.as_double());
    }
    if val.is_bool() {
        return val.as_bool().to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if val.is_undefined() {
        return "undefined".to_string();
    }
    if val.is_string() {
        return unsafe { string_data(val) }.to_string();
    }
    if val.is_object() {
        return "[object]".to_string();
    }
    String::new()
}

/// ToBoolean（ECMA-262 §7.1.2）：falsy 值仅限 undefined/null/false/±0/NaN/空串，其余为 true。
pub fn to_boolean(val: JsValue) -> bool {
    if val.is_undefined() || val.is_null() {
        return false;
    }
    if val.is_bool() {
        return val.as_bool();
    }
    if val.is_int() {
        return val.as_int() != 0;
    }
    if val.is_double() {
        let d = val.as_double();
        return !(d == 0.0 || d == -0.0 || d.is_nan());
    }
    if val.is_string() {
        return !unsafe { (*val.as_string_ptr()).is_empty() };
    }
    if val.is_object() {
        return true;
    }
    if val.is_symbol() {
        return true;
    }
    false
}

/// 两个值是否共享同一 ECMAScript 语言类型。Number 把 int 与 double tag 的值
/// 视为同一类型（都是 Number）。
fn same_type(a: JsValue, b: JsValue) -> bool {
    if a.is_string() && b.is_string() {
        return true;
    }
    if (a.is_int() || a.is_double()) && (b.is_int() || b.is_double()) {
        return true;
    }
    if a.is_bool() && b.is_bool() {
        return true;
    }
    if a.is_null() && b.is_null() {
        return true;
    }
    if a.is_undefined() && b.is_undefined() {
        return true;
    }
    if a.is_object() && b.is_object() {
        return true;
    }
    if a.is_symbol() && b.is_symbol() {
        return true;
    }
    false
}

/// IsLooselyEqual(x, y) — ECMA-262 §7.2.15（`==`）。
///
/// 省略 BigInt 步骤（引擎剪除 BigInt）。对象操作数经 ToPrimitive 强制转换，
/// 可能调用用户 `valueOf` / `toString` / `@@toPrimitive`；因此需要 `VmHost`
/// 参数与 `Result`（这些回调抛出的 TypeError 以 `Err` 传播）。
/// 注意：`Object == Symbol` 不触发 ToPrimitive（规范步骤 11/12 只覆盖
/// Number/String），直接落到 `false`。
pub fn abstract_eq<H: VmHost>(lhs: JsValue, rhs: JsValue, host: &mut H) -> Result<bool, String> {
    // 步骤 1：同类型 → 严格相等。
    if same_type(lhs, rhs) {
        return Ok(strict_equality(lhs, rhs));
    }
    // 步骤 2-3：null 与 undefined 互等。
    if (lhs.is_null() && rhs.is_undefined()) || (lhs.is_undefined() && rhs.is_null()) {
        return Ok(true);
    }
    // 步骤 5-6：Number 与 String。
    if (lhs.is_int() || lhs.is_double()) && rhs.is_string() {
        return Ok(strict_double_eq(to_number(lhs), to_number(rhs)));
    }
    if lhs.is_string() && (rhs.is_int() || rhs.is_double()) {
        return Ok(strict_double_eq(to_number(lhs), to_number(rhs)));
    }
    // 步骤 9：x 为 Boolean → 比较 ToNumber(x)。
    if lhs.is_bool() {
        return abstract_eq(JsValue::float(to_number(lhs)), rhs, host);
    }
    // 步骤 10：y 为 Boolean → 比较 ToNumber(y)。
    if rhs.is_bool() {
        return abstract_eq(lhs, JsValue::float(to_number(rhs)), host);
    }
    // 步骤 11：x 为 Number/String，y 为 Object → ToPrimitive(y)。
    if (lhs.is_int() || lhs.is_double() || lhs.is_string()) && rhs.is_object() {
        let prim = to_primitive(rhs, ToPrimitiveHint::Default, host)?;
        return abstract_eq(lhs, prim, host);
    }
    // 步骤 12：x 为 Object，y 为 Number/String → ToPrimitive(x)。
    if lhs.is_object() && (rhs.is_int() || rhs.is_double() || rhs.is_string()) {
        let prim = to_primitive(lhs, ToPrimitiveHint::Default, host)?;
        return abstract_eq(prim, rhs, host);
    }
    // 步骤 14：否则不相等。
    Ok(false)
}

/// Strict Equality Comparison（`===`，ECMA-262 §7.2.14）。
///
/// 类型不同直接为 false；同类型下 Object 按指针、其余按值比较。
pub fn strict_eq(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_int() && rhs.is_int() {
        return lhs.as_int() == rhs.as_int();
    }
    // Number 跨 int/double 表示比较：`42 === 42.0` 按 SameValue 语义应相等。
    if (lhs.is_int() || lhs.is_double()) && (rhs.is_int() || rhs.is_double()) {
        return strict_double_eq(to_f64(lhs), to_f64(rhs));
    }
    if lhs.is_double() && rhs.is_double() {
        return strict_double_eq(lhs.as_double(), rhs.as_double());
    }
    if lhs.is_bool() && rhs.is_bool() {
        return lhs.as_bool() == rhs.as_bool();
    }
    if lhs.is_null() && rhs.is_null() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return string_value_eq(lhs, rhs);
    }
    if lhs.is_object() && rhs.is_object() {
        return lhs.as_ptr() == rhs.as_ptr();
    }
    if lhs.is_symbol() && rhs.is_symbol() {
        return lhs.as_symbol_index() == rhs.as_symbol_index();
    }
    false
}

fn strict_double_eq(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return false;
    }
    a == b
}

/// Relational Comparison（`<`，ECMA-262 §7.2.13）的原始值版本。
///
/// 双字符串按字典序；否则转数值比较，任一侧为 NaN 时返回 `None`（表示比较未定义，调用方据此处理 `<`/`>`）。
pub fn relational_compare(lhs: JsValue, rhs: JsValue) -> Option<bool> {
    if lhs.is_string() && rhs.is_string() {
        let ls = unsafe { string_data(lhs) };
        let rs = unsafe { string_data(rhs) };
        return Some(ls < rs);
    }
    let l = to_number(lhs);
    let r = to_number(rhs);
    if l.is_nan() || r.is_nan() {
        return None;
    }
    l.partial_cmp(&r).map(|o| o.is_lt())
}

/// 拼接两个字符串，按已知总长度预分配容量以避免重复扩容。
pub fn string_concat(lhs: &str, rhs: &str) -> String {
    let mut s = String::with_capacity(lhs.len() + rhs.len());
    s.push_str(lhs);
    s.push_str(rhs);
    s
}

fn to_f64(val: JsValue) -> f64 {
    if val.is_int() {
        val.as_int() as f64
    } else if val.is_double() {
        val.as_double()
    } else {
        f64::NAN
    }
}

/// SameValue(x, y)（ECMA-262 §7.2.9）：与 `===` 的区别在于 NaN 视为相等、+0/-0 视为不同。
pub fn same_value(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_double() && rhs.is_double() {
        let a = lhs.as_double();
        let b = rhs.as_double();
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a == 0.0 && b == 0.0 {
            let a_neg = a.is_sign_negative();
            let b_neg = b.is_sign_negative();
            return a_neg == b_neg;
        }
        return a == b;
    }
    if lhs.is_int() && rhs.is_int() {
        return lhs.as_int() == rhs.as_int();
    }
    if (lhs.is_int() || lhs.is_double()) && (rhs.is_int() || rhs.is_double()) {
        let a = to_f64(lhs);
        let b = to_f64(rhs);
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a == 0.0 && b == 0.0 {
            return a.is_sign_negative() == b.is_sign_negative();
        }
        return a == b;
    }
    if lhs.is_bool() && rhs.is_bool() {
        return lhs.as_bool() == rhs.as_bool();
    }
    if lhs.is_null() && rhs.is_null() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return string_value_eq(lhs, rhs);
    }
    if lhs.is_object() && rhs.is_object() {
        return lhs.as_ptr() == rhs.as_ptr();
    }
    if lhs.is_symbol() && rhs.is_symbol() {
        return lhs.as_symbol_index() == rhs.as_symbol_index();
    }
    false
}

/// ToIntegerOrInfinity(argument) — ECMA-262 §7.1.4.
pub fn to_integer_or_infinity(val: JsValue) -> f64 {
    let n = to_number(val);
    if n.is_nan() || n == 0.0 {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc()
    }
}

/// ToLength(argument) — ECMA-262 §7.1.20.
pub fn to_length(val: JsValue) -> u64 {
    let n = to_number(val);
    let len = if n.is_nan() || n <= 0.0 { 0.0 } else { n.min(9_007_199_254_740_991.0) };
    len.trunc() as u64
}

/// SameValueZero(x, y) — ECMA-262 §7.2.11.
pub fn same_value_zero(lhs: JsValue, rhs: JsValue) -> bool {
    if (lhs.is_double() || lhs.is_int()) && (rhs.is_double() || rhs.is_int()) {
        let a = to_f64(lhs);
        let b = to_f64(rhs);
        if a.is_nan() && b.is_nan() {
            return true;
        }
        return a == b;
    }
    same_value(lhs, rhs)
}

/// 与 [`strict_eq`] 等价的规范层实现：双浮点走 NaN 安全比较，其余复用 [`same_value`]。
pub fn strict_equality(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_double() && rhs.is_double() {
        return strict_double_eq(lhs.as_double(), rhs.as_double());
    }
    same_value(lhs, rhs)
}

/// ToObject（ECMA-262 §7.1.13）：null/undefined 抛 TypeError，其余原始值包装为对应包装对象。
///
/// 包装对象按类型选择原型（String/Number/Boolean 原型或默认 Object 原型），原始值存入 hash props 槽。
pub fn to_object<H: VmHost>(val: JsValue, host: &mut H) -> Result<JsValue, String> {
    if val.is_object() {
        return Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        return Err(host.error_message_text("TypeError", "Cannot convert null or undefined to object"));
    }
    let world = host.session().builtin_world();
    let (proto_ptr, type_tag) = if val.is_string() {
        (P::as_ptr(&world.string_proto) as *mut JsObject, JsObject::OBJ_TYPE_STRING_OBJ)
    } else if val.is_int() || val.is_double() {
        (P::as_ptr(&world.number_proto) as *mut JsObject, JsObject::OBJ_TYPE_NUMBER_OBJ)
    } else if val.is_bool() {
        (P::as_ptr(&world.boolean_proto) as *mut JsObject, JsObject::OBJ_TYPE_BOOLEAN_OBJ)
    } else {
        (P::as_ptr(&world.object_proto) as *mut JsObject, JsObject::OBJ_TYPE_PLAIN)
    };
    let proto_val = JsValue::from_js_object(proto_ptr);
    let obj = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let obj_val = JsValue::from_js_object(obj);
    let obj_ref = unsafe { &mut *obj };
    obj_ref.type_tag = type_tag;
    obj_ref.ensure_hash_props().push(val);
    obj_ref.set_prop_count(1);
    Ok(obj_val)
}

/// Hint for the abstract ToPrimitive operation (ECMA-262 §7.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToPrimitiveHint {
    Default,
    String,
    Number,
}

impl ToPrimitiveHint {
    /// OrdinaryToPrimitive 方法名顺序提示：`Default`/`Number` → "number"，`String` → "string"。
    pub fn as_str(self) -> &'static str {
        match self {
            ToPrimitiveHint::String => "string",
            ToPrimitiveHint::Default | ToPrimitiveHint::Number => "number",
        }
    }
}

/// ToPrimitive(input, hint) per ECMA-262 §7.1.1.
///
/// 原始值原样通过。对象经 VM 的 `coerce_primitive_bounded` 强制转换——先查询
/// `obj[Symbol.toPrimitive]`，否则按 hint 顺序运行 OrdinaryToPrimitive
/// （valueOf/toString）。对象路径集中在单处，避免在此重复 OrdinaryToPrimitive。
pub fn to_primitive<H: VmHost>(val: JsValue, hint: ToPrimitiveHint, host: &mut H) -> Result<JsValue, String> {
    if !val.is_object() {
        return Ok(val);
    }
    host.coerce_primitive_bounded(val, hint == ToPrimitiveHint::String)
}

/// 带完整对象强制转换的 ToNumber(input)：对象经 ToPrimitive（number hint）
/// 处理；Symbol 值按规范抛 TypeError。
pub fn to_number_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<f64, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::Number, host)?;
    if primitive.is_symbol() {
        return Err(host.error_message_text("TypeError", "Cannot convert a Symbol value to a number"));
    }
    Ok(to_number(primitive))
}

/// 带完整对象强制转换的 ToString(input)：对象经 ToPrimitive（string hint）处理。
pub fn to_string_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<String, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::String, host)?;
    Ok(to_string(primitive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(d: f64) -> String {
        js_number_to_string(d)
    }

    #[test]
    fn number_to_string_boundaries() {
        assert_eq!(fmt(1e21), "1e+21");
        assert_eq!(fmt(1e20), "100000000000000000000");
        assert_eq!(fmt(123.45), "123.45");
        assert_eq!(fmt(0.000001), "0.000001");
        assert_eq!(fmt(0.0000001), "1e-7");
        assert_eq!(fmt(1e15), "1000000000000000");
        assert_eq!(fmt(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(fmt(0.0), "0");
        assert_eq!(fmt(-0.0), "0");
        assert_eq!(fmt(-123.45), "-123.45");
        assert_eq!(fmt(-1e21), "-1e+21");
        assert_eq!(fmt(1e16), "10000000000000000");
        assert_eq!(fmt(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(fmt(5e-324), "5e-324");
        assert_eq!(fmt(1.5e20), "150000000000000000000");
        assert_eq!(fmt(f64::NAN), "NaN");
        assert_eq!(fmt(f64::INFINITY), "Infinity");
        assert_eq!(fmt(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn parse_js_number_rules() {
        assert_eq!(parse_js_number(""), 0.0);
        assert_eq!(parse_js_number("   "), 0.0);
        assert_eq!(parse_js_number("0xa"), 10.0);
        assert_eq!(parse_js_number("0X1f"), 31.0);
        assert_eq!(parse_js_number("0b101"), 5.0);
        assert_eq!(parse_js_number("0o17"), 15.0);
        assert_eq!(parse_js_number("Infinity"), f64::INFINITY);
        assert_eq!(parse_js_number("-Infinity"), f64::NEG_INFINITY);
        assert!(parse_js_number("INFINITY").is_nan());
        assert!(parse_js_number("infinity").is_nan());
        assert!(parse_js_number("0x1g").is_nan());
        assert!(parse_js_number("-0x1").is_nan());
        assert_eq!(parse_js_number("1.5e3"), 1500.0);
        assert_eq!(parse_js_number("-0"), -0.0);
        assert!(parse_js_number("1abc").is_nan());
    }
}
