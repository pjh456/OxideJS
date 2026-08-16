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

mod runtime_api_log;

use std::cmp::Ordering;
use std::sync::Arc;

use num_traits::{ToPrimitive, Zero};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::{JsType, JsValue};

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

    /// 当前 native 调用 spill 溢出区的实参个数（寄存器窗口 253 之外的部分）。
    ///
    /// # 边界与前提
    /// - 仅在 native 实现内部、实参仍有效时读取；无溢出时为 0。
    fn native_overflow_count(&self) -> usize;
    /// 读 spill 溢出区第 `i` 个实参（0 基，相对溢出区起点）。
    ///
    /// # 边界与前提
    /// - `i` 必须小于 [`Self::native_overflow_count`]。
    fn native_overflow_at(&self, i: usize) -> JsValue;
    /// 当前 native 调用的完整实参个数（寄存器窗口 + spill 溢出区，不含 receiver）。
    ///
    /// 供支持大实参集的 builtin（如 `String.fromCodePoint`）遍历全部实参；
    /// 未迁移的 builtin 仍按 `args` 索引读寄存器，行为不变。
    fn native_arg_count(&self, args: &[u8]) -> usize {
        args.len().saturating_sub(1) + self.native_overflow_count()
    }
    /// 第 `idx` 个实参（0 基，不含 receiver）：窗口内读寄存器，窗口外读 spill 溢出区。
    ///
    /// # 边界与前提
    /// - `idx` 必须小于 [`Self::native_arg_count`]。
    fn native_arg_at(&self, args: &[u8], idx: usize) -> JsValue {
        let window = args.len().saturating_sub(1);
        if idx < window {
            self.reg(args[idx + 1])
        } else {
            self.native_overflow_at(idx - window)
        }
    }

    // 对象分配 / 字符串创建
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject;
    fn new_string(&mut self, s: &str) -> JsValue;
    /// move 接收 `String` 创建会话字符串，避免一次整串克隆。
    fn new_string_owned(&mut self, s: String) -> JsValue;
    /// 取 ASCII 单字符的永久字符串值：命中返回共享 perm 串（零分配、可指针
    /// 短路比较），非 ASCII 返回 `None` 由调用方回落普通字符串创建。
    ///
    /// # 注意事项
    /// `&self` 可借用期调用；`None` 回落 `new_string` 是 `&mut` 路径，须先结束
    /// 本次 `&self` 借用（返回值即时消费即可）。
    fn single_char(&self, ch: char) -> Option<JsValue> {
        if ch.is_ascii() {
            Some(JsValue::string(oxide_kernel::string_forge::single_char_ptr(ch as u8)))
        } else {
            None
        }
    }
    /// 借出字符串值的文本内容，生命周期绑定到 `&self` 借用。
    ///
    /// perm 字符串由内核持有、永不释放；session 字符串只在 `&mut self` 路径
    /// （`new_string`/`new_string_owned`/`maybe_collect_session_gc`）释放，`&self`
    /// 借用与 `&mut` 互斥由编译器强制，借用期内该字符串不会回收。实现内部
    /// `unsafe` 解引用，调用方须保证 `val` 为字符串值。
    fn string_ref(&self, val: JsValue) -> &str;
    /// 分配 BigInt 值（num_bigint::BigInt box 登记到 VM，返回携带指针的 `JsValue`）。
    fn new_bigint(&mut self, v: num_bigint::BigInt) -> JsValue;
    /// 读取 BigInt 值；调用方须保证 `val.is_bigint()`。
    fn bigint_value(&mut self, val: JsValue) -> &num_bigint::BigInt;

    // 内核访问器
    fn kernel_core(&self) -> &Arc<KernelCore>;
    fn session(&self) -> &KernelSession;
    fn epoch(&self) -> &Epoch;

    // 属性解析
    fn property_key_si(&mut self, val: JsValue) -> u32;
    /// 字符串→键规范化：规范数字串（`"5"`）映射整数键，其余 intern 字符串键。
    /// 供建键入口（fromEntries/json/rest excluded）与 `property_key_si` 的字符串分支统一口径。
    fn string_key_si(&mut self, s: &str) -> u32;
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
    /// 恢复被忽略调用暂存的原始抛出值（与 [`Self::take_uncaught_value`] 配对）。
    ///
    /// # 注意事项
    /// 忽略调用（如 IteratorClose 的 `return()`）抛错时不得让自身值覆盖槽——
    /// 调用前暂存、调用后恢复，保证在途异常值跨忽略调用存活。
    fn restore_uncaught_value(&mut self, value: Option<JsValue>);

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
    /// 动态编译脚本（`eval` 字符串模式）：按脚本模式编译，var/函数声明落全局对象。
    /// 编译或解析失败返回 `Err`，由调用方转为 `SyntaxError`。
    fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String>;
    /// `None` 表示无描述（`Symbol()`/`Symbol(undefined)`），`Some(desc)` 为字符串描述。
    fn symbol_intern(&mut self, desc: Option<String>) -> u32;
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

/// 借出字符串值的文本内容，生命周期为 `'static`。仅限同一函数内即时消费。
///
/// # Safety
/// 调用方必须保证：从取用返回值到最后一次使用之间，不发生任何可能释放该字符串
/// 的操作（分配、GC、`full_reset`）——session 字符串在执行期 GC 或 reset 时会被
/// 释放，跨分配点持有引用即悬垂。需要跨分配点消费的场景改用 [`VmHost::string_ref`]
/// 或公开的 [`to_string`] owned 路径。
#[inline]
pub(crate) unsafe fn string_data(val: JsValue) -> &'static str {
    (*val.as_string_ptr()).as_str()
}

/// 按内容比较两个字符串 `JsValue` 是否相等。
///
/// 先做指针级短路（同一 interned 字符串必等），否则比较 `JsString` 的整块文本
/// （rope 未扁平化时经 `as_str` 惰性扁平化；内容比较对"rope vs 同内容不同指针"
/// 也正确）。
#[inline]
pub fn string_value_eq(a: JsValue, b: JsValue) -> bool {
    if a.as_string_ptr() == b.as_string_ptr() {
        return true;
    }
    let sa = unsafe { &*a.as_string_ptr() };
    let sb = unsafe { &*b.as_string_ptr() };
    sa.as_str() == sb.as_str()
}

/// BigInt 转 f64 的近似转换（连续整数用 to_u64，大数用 Display 解析）。
#[inline]
pub fn bigint_to_f64(v: &num_bigint::BigInt) -> f64 {
    if let Some(u) = v.to_u64() {
        return u as f64;
    }
    v.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
}

/// 借出 BigInt 值。
///
/// # Safety
/// `val` 必须是 BigInt `JsValue`，且其 `num_bigint::BigInt` box 存活（VM 在 full_reset 前保证）。
#[inline]
pub unsafe fn bigint_data(val: JsValue) -> &'static num_bigint::BigInt {
    &*val.as_bigint_ptr()
}

/// BigInt 的十进制字符串表示（num_bigint::BigInt → 十进制）。
///
/// 与 ECMA-262 `Number::toString` 无关：BigInt 恒以十进制输出，负号前缀。
#[inline]
pub fn bigint_to_string(v: &num_bigint::BigInt) -> String {
    v.to_string()
}

/// 对原始值执行 ECMAScript ToNumber 的快速路径（不触发对象 coercion）。
///
/// Number/String/Boolean/null 按规范转换；undefined 与不可解析字符串为 `NaN`；
/// Object 与 Symbol 不在此处理，返回 `NaN`（对象需走 [`to_number_full`]）。
pub fn to_number(val: JsValue) -> f64 {
    match val.js_type() {
        JsType::Int => val.as_int() as f64,
        JsType::Double => val.as_double(),
        JsType::Bool => {
            if val.as_bool() {
                1.0
            } else {
                0.0
            }
        }
        JsType::Null => 0.0,
        JsType::Undefined => f64::NAN,
        JsType::String => {
            let s = unsafe { string_data(val) };
            parse_js_number(s)
        }
        JsType::BigInt => {
            // BigInt → Number 近似转换：i128 超出 f64 精度时舍入为近似值。
            // 规范路径（显式 Number(bigint) / 位运算）在 builtins / dispatch 层精确处理。
            bigint_to_f64(unsafe { bigint_data(val) })
        }
        JsType::Object | JsType::Symbol => f64::NAN,
    }
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
    let b = t.as_bytes();
    // 前缀探测按字节直接比较（ASCII 大小写不敏感），避免每次转换分配 lowercase。
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') {
        return parse_radix_int(&t[2..], 16);
    }
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'o' || b[1] == b'O') {
        return parse_radix_int(&t[2..], 8);
    }
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'b' || b[1] == b'B') {
        return parse_radix_int(&t[2..], 2);
    }
    if t == "Infinity" || t == "+Infinity" {
        return f64::INFINITY;
    }
    if t == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    // 十进制语法仅允许数字、符号、小数点与指数 e；含其它字符的令牌（如
    // inf/nan 变体）不是合法 StringNumericLiteral，一律 NaN。
    if !b
        .iter()
        .all(|&c| c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' || c == b'e' || c == b'E')
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
/// 直接调用 [`write_number_into`] 写入 32 字节预分配的缓冲区（ryu 输出上界
/// ~24 字节 + 负号），单次分配无扩容。
///
/// # 边界与前提
/// - NaN → "NaN"，±∞ → "±Infinity"，±0 → "0"
/// - 32 字节容量恒够：最长输出 `-1.7976931348623157e+308`（25 字节）。
pub fn js_number_to_string(d: f64) -> String {
    let mut out = String::with_capacity(32);
    write_number_into(d, &mut out);
    out
}

/// 把 f64 格式化为 ECMA-262 Number::toString 的文本，追加到 `out`。
///
/// 对有限非零数，先经 ryu 最短表示取出有效数字与十进制指数（`format_finite`
/// 输出形如 `"123.45"` / `"1e21"` / `"1000000000000000.0"`），再按规范分段
/// 重建：指数 `-6 < n <= 21` 时用定点表示，其余用科学计数法 `d.ddde±e`。
///
/// # 步骤
/// 1. NaN/±∞/±0 直接写静态串；负数先写 `-`。
/// 2. 整数 double 快路径：`fract()==0` 且 `|d| < 2^53` 时整数直写（i64 itoa），
///    跳过 ryu 与重建。
/// 3. 含 `e` 时按字节解析有效数字与指数（ryu 输出全 ASCII，指数不带 `+`），
///    有效数字收栈上数组后按四段规则一次写齐。
/// 4. 不含 `e` 的定点形式仅需去掉整数末尾的 `.0` 后缀。
///
/// # 边界与前提
/// - 整数快路径上限取 2^53 而非 1e21：2^53 内整数在 f64 中精确且十进制有效
///   位数 ≤ 16，整数直写与最短表示一致；[2^53, 1e21) 的整数 double 精确值
///   可能与规范最短表示分歧（如 2^55 精确值 36028797018963968，规范输出
///   36028797018963970），故该区间保留 ryu 路径。1e21/1e22 等自然被排除。
/// - ryu 输出全 ASCII 且指数无 `+`；有效数字 ≤ 17 位，栈缓冲 24 字节恒够。
///
/// # 副作用
/// - 追加到 `out`，不新建中间字符串、不做堆分配（`out` 扩容除外）。
pub fn write_number_into(d: f64, out: &mut String) {
    if d.is_nan() {
        out.push_str("NaN");
        return;
    }
    if d.is_infinite() {
        out.push_str(if d.is_sign_positive() { "Infinity" } else { "-Infinity" });
        return;
    }
    if d == 0.0 {
        out.push('0');
        return;
    }
    let neg = d.is_sign_negative();
    let abs = d.abs();
    // 有限非零负数先追加负号（追加语义：插到当前数值之前，不扰动 out 已有前缀）。
    if neg {
        out.push('-');
    }
    // 整数 double 快路径：无小数部分且在 2^53 内（精确整数，直写与最短表示一致）。
    if abs.fract() == 0.0 && abs < 9_007_199_254_740_992.0 {
        use std::fmt::Write;
        let _ = write!(out, "{}", abs as i64);
        return;
    }
    let mut buf = ryu::Buffer::new();
    let s = buf.format_finite(abs);
    if let Some(e_pos) = s.find('e') {
        // 有效数字收栈上数组（ryu 输出全 ASCII），跳过 '.' 逐字节拷贝。
        let mut digits = [0u8; 24];
        let mut k = 0usize;
        for &b in s.as_bytes()[..e_pos].iter() {
            if b != b'.' {
                digits[k] = b;
                k += 1;
            }
        }
        debug_assert!(k <= 24, "ryu 有效数字不超过 24 字节");
        let exp: i32 = s[e_pos + 1..].parse().unwrap_or(0);
        let n = exp + 1;
        if n > -6 && n <= 21 {
            // 定点：ryu 在此范围输出指数形式但 JS 要求十进制。
            if n <= 0 {
                out.push_str("0.");
                for _ in 0..(-n) {
                    out.push('0');
                }
                out.push_str(std::str::from_utf8(&digits[..k]).expect("有效数字恒为 ASCII"));
            } else if k as i32 <= n {
                out.push_str(std::str::from_utf8(&digits[..k]).expect("有效数字恒为 ASCII"));
                for _ in 0..(n - k as i32) {
                    out.push('0');
                }
            } else {
                out.push_str(std::str::from_utf8(&digits[..n as usize]).expect("有效数字恒为 ASCII"));
                out.push('.');
                out.push_str(std::str::from_utf8(&digits[n as usize..k]).expect("有效数字恒为 ASCII"));
            }
        } else {
            // 科学计数：首位 + (可选 "." + 剩余) + e[+/-]指数。指数 ≤ 3 位，write! 直写。
            out.push(digits[0] as char);
            if k > 1 {
                out.push('.');
                out.push_str(std::str::from_utf8(&digits[1..k]).expect("有效数字恒为 ASCII"));
            }
            out.push('e');
            if n > 1 {
                out.push('+');
            }
            use std::fmt::Write;
            let _ = write!(out, "{}", n - 1);
        }
    } else if let Some(stripped) = s.strip_suffix(".0") {
        out.push_str(stripped);
    } else {
        out.push_str(s);
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
        write_number_into(val.as_double(), buf);
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
    if val.is_bigint() {
        use std::fmt::Write;
        let _ = write!(buf, "{}", unsafe { bigint_data(val) });
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
    if val.is_bigint() {
        return bigint_to_string(unsafe { bigint_data(val) });
    }
    if val.is_object() {
        return "[object]".to_string();
    }
    String::new()
}

/// ToBoolean（ECMA-262 §7.1.2）：falsy 值仅限 undefined/null/false/±0/NaN/空串，其余为 true。
pub fn to_boolean(val: JsValue) -> bool {
    match val.js_type() {
        JsType::Undefined | JsType::Null => false,
        JsType::Bool => val.as_bool(),
        JsType::Int => val.as_int() != 0,
        JsType::Double => {
            let d = val.as_double();
            // ±0 与 NaN 均为 falsy（IEEE 中 0.0 == -0.0，符号位无需单独判断）。
            !(d == 0.0 || d.is_nan())
        }
        JsType::String => !unsafe { (*val.as_string_ptr()).is_empty() },
        JsType::BigInt => !unsafe { bigint_data(val) }.is_zero(),
        JsType::Object | JsType::Symbol => true,
    }
}

/// IsLooselyEqual(x, y) — ECMA-262 §7.2.15（`==`）。
///
/// 同类型委托严格相等；null 与 undefined 互等；BigInt/Number/String 两两
/// 转数值比较；Boolean 操作数先 ToNumber；原始值 vs Object 经 ToPrimitive
/// 强制转换后重入（Symbol 与 Object 同样触发，规范步骤 10-11）。对象路径
/// 可能调用用户 `valueOf` / `toString` / `@@toPrimitive`，因此需要 `VmHost`
/// 参数与 `Result`（回调抛出的 TypeError 以 `Err` 传播）。
pub fn abstract_eq<H: VmHost>(lhs: JsValue, rhs: JsValue, host: &mut H) -> Result<bool, String> {
    let tl = lhs.js_type();
    let tr = rhs.js_type();
    // 同类型 → 严格相等。
    if tl == tr {
        return Ok(strict_equality(lhs, rhs));
    }
    match (tl, tr) {
        // null 与 undefined 互等。
        (JsType::Null, JsType::Undefined) | (JsType::Undefined, JsType::Null) => Ok(true),
        // Number 混合表示（int vs double）：同属 ECMAScript Number，按严格相等。
        (JsType::Int | JsType::Double, JsType::Int | JsType::Double) => Ok(strict_equality(lhs, rhs)),
        // BigInt 与 Number：转 f64 比较（精度内精确；超出精度近似）。
        (JsType::BigInt, JsType::Int | JsType::Double) => {
            Ok(bigint_to_f64(unsafe { bigint_data(lhs) }) == to_number(rhs))
        }
        (JsType::Int | JsType::Double, JsType::BigInt) => {
            Ok(to_number(lhs) == bigint_to_f64(unsafe { bigint_data(rhs) }))
        }
        // BigInt 与 String：字符串解析为数字后比较。
        (JsType::BigInt, JsType::String) => {
            let r = parse_js_number(unsafe { string_data(rhs) });
            Ok(bigint_to_f64(unsafe { bigint_data(lhs) }) == r)
        }
        (JsType::String, JsType::BigInt) => {
            let l = parse_js_number(unsafe { string_data(lhs) });
            Ok(l == bigint_to_f64(unsafe { bigint_data(rhs) }))
        }
        // Number 与 String：字符串经 ToNumber 后按严格数值比较。
        (JsType::Int | JsType::Double, JsType::String) | (JsType::String, JsType::Int | JsType::Double) => {
            Ok(strict_double_eq(to_number(lhs), to_number(rhs)))
        }
        // Boolean 操作数：先 ToNumber 再重入。int(0/1) 轻量编码，省 float
        // 装箱与 to_number 链；int 属于 Number，重入后各数值分支全部认。
        (JsType::Bool, _) => abstract_eq(JsValue::int(lhs.as_bool() as i32), rhs, host),
        (_, JsType::Bool) => abstract_eq(lhs, JsValue::int(rhs.as_bool() as i32), host),
        // Object 与原始值（Number/String/BigInt/Symbol）：ToPrimitive 后重入。
        (JsType::Object, JsType::Int | JsType::Double | JsType::String | JsType::BigInt | JsType::Symbol) => {
            let prim = to_primitive(lhs, ToPrimitiveHint::Default, host)?;
            abstract_eq(prim, rhs, host)
        }
        (JsType::Int | JsType::Double | JsType::String | JsType::BigInt | JsType::Symbol, JsType::Object) => {
            let prim = to_primitive(rhs, ToPrimitiveHint::Default, host)?;
            abstract_eq(lhs, prim, host)
        }
        // 其余组合不相等（含 null/undefined 与其它类型、Object/Object 已由同类型分支处理）。
        _ => Ok(false),
    }
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
    if lhs.is_bigint() && rhs.is_bigint() {
        let l = unsafe { bigint_data(lhs) };
        let r = unsafe { bigint_data(rhs) };
        return Some(l < r);
    }
    // BigInt 与 Number 混合：Number 为整数且落在 i128 范围时转 i128 精确比较，
    // 否则（非整数/超出范围/NaN/±Infinity）转 f64 比较（NaN 结果为未定义）。
    if lhs.is_bigint() && (rhs.is_int() || rhs.is_double()) {
        if rhs.is_double() && rhs.as_double().is_nan() {
            return None;
        }
        let l = unsafe { bigint_data(lhs) };
        return Some(bigint_cmp_number(l, rhs) == Ordering::Less);
    }
    if (lhs.is_int() || lhs.is_double()) && rhs.is_bigint() {
        if lhs.is_double() && lhs.as_double().is_nan() {
            return None;
        }
        let r = unsafe { bigint_data(rhs) };
        // lhs(Number) < rhs(BigInt) ⇔ 反向比较（lhs 作为"number 侧"）。
        return Some(number_cmp_bigint(lhs, r) == Ordering::Less);
    }
    // BigInt 与 String/Boolean 混合：转 Number 后比较。
    if lhs.is_bigint() || rhs.is_bigint() {
        let l = to_number(lhs);
        let r = to_number(rhs);
        if l.is_nan() || r.is_nan() {
            return None;
        }
        return l.partial_cmp(&r).map(|o| o.is_lt());
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

/// BigInt 与 Number 的序比较：Number 为整数且落于 i128 范围时精确比较，
/// 否则退化为 f64 近似比较（非有限/超出 i128 范围时）。
fn bigint_cmp_number(big: &num_bigint::BigInt, num: JsValue) -> Ordering {
    number_cmp_bigint(num, big).reverse()
}

/// Number 与 BigInt 的序比较（`num` 与 `big` 的 `<`/`=`/`>`）。
fn number_cmp_bigint(num: JsValue, big: &num_bigint::BigInt) -> Ordering {
    if num.is_int() {
        return (num.as_int() as i128).cmp(&big.to_i128().unwrap_or(if big.sign() == num_bigint::Sign::Minus {
            i128::MIN
        } else {
            i128::MAX
        }));
    }
    let d = num.as_double();
    if d.is_nan() {
        return Ordering::Greater;
    }
    if d.is_infinite() {
        return if d.is_sign_positive() { Ordering::Greater } else { Ordering::Less };
    }
    let truncated = d.trunc();
    if truncated != d || truncated.abs() > i128::MAX as f64 {
        return (d).partial_cmp(&bigint_to_f64(big)).unwrap_or(Ordering::Equal);
    }
    (truncated as i128).cmp(&big.to_i128().unwrap_or(if big.sign() == num_bigint::Sign::Minus {
        i128::MIN
    } else {
        i128::MAX
    }))
}

/// SameValue(x, y)（ECMA-262 §7.2.9）：与 `===` 的区别在于 NaN 视为相等、+0/-0 视为不同。
pub fn same_value(lhs: JsValue, rhs: JsValue) -> bool {
    match (lhs.js_type(), rhs.js_type()) {
        (JsType::Double, JsType::Double) => {
            let a = lhs.as_double();
            let b = rhs.as_double();
            if a.is_nan() && b.is_nan() {
                return true;
            }
            if a == 0.0 && b == 0.0 {
                return a.is_sign_negative() == b.is_sign_negative();
            }
            a == b
        }
        (JsType::Int, JsType::Int) => lhs.as_int() == rhs.as_int(),
        // Number 混合表示（int vs double）：SameValue 的 ±0/NaN 特判原样保留。
        (JsType::Int | JsType::Double, JsType::Int | JsType::Double) => {
            let a = to_f64(lhs);
            let b = to_f64(rhs);
            if a.is_nan() && b.is_nan() {
                return true;
            }
            if a == 0.0 && b == 0.0 {
                return a.is_sign_negative() == b.is_sign_negative();
            }
            a == b
        }
        (JsType::Bool, JsType::Bool) => lhs.as_bool() == rhs.as_bool(),
        (JsType::Null, JsType::Null) => true,
        (JsType::Undefined, JsType::Undefined) => true,
        (JsType::String, JsType::String) => string_value_eq(lhs, rhs),
        (JsType::Object, JsType::Object) => lhs.as_ptr() == rhs.as_ptr(),
        (JsType::Symbol, JsType::Symbol) => lhs.as_symbol_index() == rhs.as_symbol_index(),
        // SAFETY: bigint 指针由 JsValue::bigint 构造，指向 VM 登记的存活 box。
        (JsType::BigInt, JsType::BigInt) => unsafe { bigint_data(lhs) == bigint_data(rhs) },
        _ => false,
    }
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
    match (lhs.js_type(), rhs.js_type()) {
        (JsType::Int, JsType::Int) => lhs.as_int() == rhs.as_int(),
        (JsType::Double, JsType::Double) => {
            let a = lhs.as_double();
            let b = rhs.as_double();
            if a.is_nan() && b.is_nan() {
                return true;
            }
            a == b
        }
        // Number 混合表示：NaN 视为相等，+0/-0 相等（Rust f64 中 +0 == -0）。
        (JsType::Int | JsType::Double, JsType::Int | JsType::Double) => {
            let a = to_f64(lhs);
            let b = to_f64(rhs);
            if a.is_nan() && b.is_nan() {
                return true;
            }
            a == b
        }
        _ => same_value(lhs, rhs),
    }
}

/// Strict Equality Comparison（`===`，ECMA-262 §7.2.14）。
///
/// 类型不同直接为 false；同类型下 Number（int/double 两种表示）按数值
/// 比较（NaN 恒 false、+0/-0 相等，含 int/double 混合——`0 === -0` 为 true），
/// Object 按指针、其余按值比较。
pub fn strict_equality(lhs: JsValue, rhs: JsValue) -> bool {
    match (lhs.js_type(), rhs.js_type()) {
        (JsType::Int, JsType::Int) => lhs.as_int() == rhs.as_int(),
        (JsType::Double, JsType::Double) => strict_double_eq(lhs.as_double(), rhs.as_double()),
        // Number 跨 int/double 表示：`42 === 42.0`、`0 === -0` 均按数值语义。
        (JsType::Int | JsType::Double, JsType::Int | JsType::Double) => strict_double_eq(to_f64(lhs), to_f64(rhs)),
        (JsType::Bool, JsType::Bool) => lhs.as_bool() == rhs.as_bool(),
        (JsType::Null, JsType::Null) => true,
        (JsType::Undefined, JsType::Undefined) => true,
        (JsType::String, JsType::String) => string_value_eq(lhs, rhs),
        (JsType::Object, JsType::Object) => lhs.as_ptr() == rhs.as_ptr(),
        (JsType::Symbol, JsType::Symbol) => lhs.as_symbol_index() == rhs.as_symbol_index(),
        // SAFETY: bigint 指针由 JsValue::bigint 构造，指向 VM 登记的存活 box。
        (JsType::BigInt, JsType::BigInt) => unsafe { bigint_data(lhs) == bigint_data(rhs) },
        _ => false,
    }
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
    } else if val.is_bigint() {
        (P::as_ptr(&world.bigint_proto) as *mut JsObject, JsObject::OBJ_TYPE_PLAIN)
    } else if val.is_symbol() {
        (P::as_ptr(&world.symbol_proto) as *mut JsObject, JsObject::OBJ_TYPE_SYMBOL_OBJ)
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

/// 带完整对象强制转换的 ToString(input)：对象经 ToPrimitive（string hint）处理；
/// Symbol 值按规范（§7.1.17）抛 TypeError。
pub fn to_string_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<String, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::String, host)?;
    if primitive.is_symbol() {
        return Err(host.error_message_text("TypeError", "Cannot convert a Symbol value to a string"));
    }
    Ok(to_string(primitive))
}

/// 带完整对象强制转换的 ToBigInt(input)（§7.1.14）：BigInt 原样；Boolean → 0/1；
/// String 走 StringToBigInt；Number/Symbol/undefined/null → TypeError；对象经
/// ToPrimitive（default hint）后递归处理。
pub fn to_bigint_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<JsValue, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::Default, host)?;
    if primitive.is_bigint() {
        return Ok(primitive);
    }
    if primitive.is_bool() {
        let v = if primitive.as_bool() { 1 } else { 0 };
        return Ok(host.new_bigint(num_bigint::BigInt::from(v)));
    }
    if primitive.is_int() || primitive.is_double() {
        return Err(host.error_message_text("TypeError", "Cannot convert a Number value to a BigInt"));
    }
    if primitive.is_string() {
        return string_to_bigint_full(host, &to_string(primitive));
    }
    // undefined / null / symbol。
    Err(host.error_message_text("TypeError", "Cannot convert value to a BigInt"))
}

/// StringToBigInt（§7.1.14 步骤）：去首尾空白，可选 +/- 号与 0x/0o/0b 前缀；
/// 空串 → 0n；非法 → SyntaxError。
fn string_to_bigint_full<H: VmHost>(host: &mut H, s: &str) -> Result<JsValue, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(host.new_bigint(num_bigint::BigInt::from(0)));
    }
    let (neg, rest) = if let Some(r) = trimmed.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = trimmed.strip_prefix('+') {
        (false, r)
    } else {
        (false, trimmed)
    };
    // StringIntegerLiteral：符号只允许出现在纯十进制前；0x/0o/0b 前缀前带
    // +/-（如 "-0x1"）属于非法语法。
    let (radix, digits) = if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        if neg {
            return Err(
                host.error_message_text("SyntaxError", "Cannot convert string to BigInt: invalid integer literal")
            );
        }
        (16u32, hex)
    } else if let Some(oct) = rest.strip_prefix("0o").or_else(|| rest.strip_prefix("0O")) {
        if neg {
            return Err(
                host.error_message_text("SyntaxError", "Cannot convert string to BigInt: invalid integer literal")
            );
        }
        (8u32, oct)
    } else if let Some(bin) = rest.strip_prefix("0b").or_else(|| rest.strip_prefix("0B")) {
        if neg {
            return Err(
                host.error_message_text("SyntaxError", "Cannot convert string to BigInt: invalid integer literal")
            );
        }
        (2u32, bin)
    } else {
        (10u32, rest)
    };
    if digits.is_empty() {
        return Err(host.error_message_text("SyntaxError", "Cannot convert string to BigInt: invalid integer literal"));
    }
    let m = num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix).ok_or_else(|| {
        host.error_message_text("SyntaxError", "Cannot convert string to BigInt: invalid integer literal")
    })?;
    Ok(host.new_bigint(if neg { -m } else { m }))
}

/// `String()` 构造器的字符串转换（§21.1.1.1）：Symbol 值（及本引擎以空对象表示的
/// well-known symbol）返回描述串 `Symbol(desc)`；其余走完整 ToString。
pub fn to_string_for_string_constructor<H: VmHost>(val: JsValue, host: &mut H) -> Result<String, String> {
    if val.is_object() {
        if let Some(id) = well_known_symbol_id(host, val.as_js_object_ptr()) {
            if let Some(name) = well_known_symbol_name(id) {
                return Ok(format!("Symbol({name})"));
            }
        }
    }
    let primitive = to_primitive(val, ToPrimitiveHint::String, host)?;
    if primitive.is_symbol() {
        let desc = host.symbol_description(primitive.as_symbol_index()).unwrap_or("");
        return Ok(format!("Symbol({desc})"));
    }
    Ok(to_string(primitive))
}

/// Well-known symbols are stored as empty objects in the builtin world; map an object
/// pointer back to its well-known symbol id (0..WELL_KNOWN_SYMBOL_COUNT) if it is one.
pub fn well_known_symbol_id<H: VmHost + ?Sized>(host: &H, ptr: *mut JsObject) -> Option<u32> {
    if ptr.is_null() {
        return None;
    }
    let world = host.session().builtin_world();
    if std::ptr::eq(ptr, world.sym_iterator.as_ptr()) {
        Some(0)
    } else if std::ptr::eq(ptr, world.sym_match.as_ptr()) {
        Some(1)
    } else if std::ptr::eq(ptr, world.sym_replace.as_ptr()) {
        Some(2)
    } else if std::ptr::eq(ptr, world.sym_search.as_ptr()) {
        Some(3)
    } else if std::ptr::eq(ptr, world.sym_split.as_ptr()) {
        Some(4)
    } else if std::ptr::eq(ptr, world.sym_to_primitive.as_ptr()) {
        Some(5)
    } else if std::ptr::eq(ptr, world.sym_has_instance.as_ptr()) {
        Some(6)
    } else if std::ptr::eq(ptr, world.sym_match_all.as_ptr()) {
        Some(7)
    } else if std::ptr::eq(ptr, world.sym_async_iterator.as_ptr()) {
        Some(8)
    } else if std::ptr::eq(ptr, world.sym_to_string_tag.as_ptr()) {
        Some(9)
    } else if std::ptr::eq(ptr, world.sym_species.as_ptr()) {
        Some(10)
    } else if std::ptr::eq(ptr, world.sym_async_dispose.as_ptr()) {
        Some(11)
    } else if std::ptr::eq(ptr, world.sym_dispose.as_ptr()) {
        Some(12)
    } else {
        None
    }
}

/// Descriptive name of a well-known symbol id, e.g. `Symbol.toStringTag` for id 9.
pub fn well_known_symbol_name(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "Symbol.iterator",
        1 => "Symbol.match",
        2 => "Symbol.replace",
        3 => "Symbol.search",
        4 => "Symbol.split",
        5 => "Symbol.toPrimitive",
        6 => "Symbol.hasInstance",
        7 => "Symbol.matchAll",
        8 => "Symbol.asyncIterator",
        9 => "Symbol.toStringTag",
        10 => "Symbol.species",
        11 => "Symbol.asyncDispose",
        12 => "Symbol.dispose",
        _ => return None,
    })
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
        assert_eq!(fmt(1e22), "1e+22");
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
        // 整数 double 快路径：2^53 内直写（与最短表示一致）。
        assert_eq!(fmt(42.0), "42");
        assert_eq!(fmt(-42.0), "-42");
        assert_eq!(fmt(123.0), "123");
        assert_eq!(fmt(2.0), "2");
        assert_eq!(fmt(1000000.0), "1000000");
        assert_eq!(fmt(-1000000.0), "-1000000");
        // 2^53 边界：等于 2^53 落 ryu 路径，输出仍为定点。
        assert_eq!(fmt(9_007_199_254_740_992.0), "9007199254740992");
        // 2^63：精确值 9223372036854775808 的最短表示为 17 位舍入
        // "9223372036854776000"（规范输出，非精确值）。
        assert_eq!(fmt(2f64.powi(63)), "9223372036854776000");
    }

    #[test]
    fn push_to_string_matches_to_string() {
        let samples: Vec<f64> = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            42.0,
            -42.0,
            std::f64::consts::PI,
            1e15,
            1e16,
            1e20,
            1e21,
            1e22,
            9_007_199_254_740_992.0,
            2f64.powi(63),
            f64::MAX,
            5e-324,
            0.1 + 0.2,
            123.45,
            -123.45,
            1e-7,
            0.000001,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for &d in &samples {
            let mut buf = String::new();
            push_to_string(JsValue::float(d), &mut buf);
            assert_eq!(buf, to_string(JsValue::float(d)), "double {d} 的 push/to_string 输出不一致");
            assert_eq!(buf, js_number_to_string(d), "double {d} 的 push/js_number_to_string 输出不一致");
        }
    }

    #[test]
    fn push_to_string_negative_double_appends_to_nonempty_buffer() {
        // 追加语义回归：非空前缀下负数 double 的负号必须紧跟当前数值，不能插到
        // 整个缓冲最前。覆盖整数快路径、ryu 定点（≥2^53）、ryu 科学计数与边界值。
        let samples: Vec<f64> = vec![
            -42.0,                    // 整数快路径
            -1.5,                     // ryu 定点
            -9_007_199_254_740_994.0, // 2^53+2：≥2^53 可精确表示整数，落 ryu 路径
            -2f64.powi(63),           // ryu 科学计数（规范最短表示）
            -1e21,                    // ryu 科学计数
            -1e-7,                    // ryu 科学计数（小指数）
            -f64::MAX,
            -5e-324,
        ];
        for &d in &samples {
            let mut buf = String::from("pre");
            push_to_string(JsValue::float(d), &mut buf);
            let expected = format!("pre{}", js_number_to_string(d));
            assert_eq!(buf, expected, "非空前缀 + double {d} 的追加语义破坏，实际 {buf}");
        }
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
