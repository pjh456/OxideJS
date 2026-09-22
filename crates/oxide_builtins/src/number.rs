use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

/// 整值且 i32 域内的有限数转 int 表示；-0 排除
/// （int 表示丢符号，规范要求 `Number(-0)` 得 -0）。
fn i32_of_integral(n: f64) -> Option<i32> {
    if n == 0.0 && n.is_sign_negative() {
        return None;
    }
    if n.fract() == 0.0 && n.is_finite() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        Some(n as i32)
    } else {
        None
    }
}

/// JS `Number()` 构造逻辑：把参数按 ToNumber 语义转换。
/// 普通调用返回原始 number（整数走 int 表示）；new 语义返回 `[[NumberData]]` 包装对象。
pub fn number_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() > 1 {
        let raw = vm.reg(args[1]);
        // ToPrimitive 先解盒：BigInt 原始值走 lossy 转换（BigIntToNumber 不抛错，
        // 显式 `Number(bigint)` 是规范唯一合法入口）；其余经 bounded ToNumber
        // 入口（Symbol 抛 TypeError）。
        let prim = match vm.coerce_primitive_bounded(raw, false) {
            Ok(p) => p,
            Err(_) => {
                // 对象经 ToPrimitive 转换时 toString/valueOf 可抛异常，须原样传播原始异常。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        };
        if prim.is_bigint() {
            oxide_runtime_api::bigint_to_f64(unsafe { oxide_runtime_api::bigint_data(prim) })
        } else if prim.is_symbol() {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a number"));
        } else {
            oxide_runtime_api::to_number(prim)
        }
    } else {
        0.0
    };
    let number_proto = vm.session().builtin_world().number_proto.as_ptr() as *mut oxide_types::object::JsObject;
    let is_ctor = if let Some(this_reg) = args.first().copied() {
        let this_val = vm.reg(this_reg);
        if this_val.is_object() {
            let ptr = this_val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                let proto_ptr = unsafe { (*ptr).proto().as_js_object_ptr() };
                !proto_ptr.is_null() && std::ptr::eq(proto_ptr, number_proto)
            }
        } else {
            false
        }
    } else {
        false
    };

    if is_ctor {
        let this_val = vm.reg(args[0]);
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        obj.type_tag = oxide_types::object::JsObject::OBJ_TYPE_NUMBER_OBJ;
        let boxed = i32_of_integral(n).map(JsValue::int).unwrap_or_else(|| JsValue::float(n));
        obj.set_boxed_value(boxed);
        return NativeResult::Ok(this_val);
    }

    NativeResult::Ok(i32_of_integral(n).map(JsValue::int).unwrap_or_else(|| JsValue::float(n)))
}

/// `Number.isNaN`：参数严格等于 NaN 才返回 true（不做隐式类型转换）。
///
/// 步 1 为严格判型：非 Number 原始类型（含 Number 对象）直接 false，
/// 不走 ToNumber 强转（否则 "NaN"/对象装箱会误报 true）。
pub fn number_is_nan<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.reg(args[1]);
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    NativeResult::Ok(JsValue::bool(oxide_runtime_api::to_number(val).is_nan()))
}

/// `Number.isFinite`：参数为有限数才返回 true（不做隐式类型转换）。
///
/// 步 1 为严格判型：非 Number 原始类型（含 Number 对象）直接 false，
/// 不走 ToNumber 强转（否则 "1" 等会误报 true）。
pub fn number_is_finite<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.reg(args[1]);
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    NativeResult::Ok(JsValue::bool(oxide_runtime_api::to_number(val).is_finite()))
}

/// 判定 parseInt/parseFloat 修剪时要剥掉的空白字符（规范 WhiteSpace 与
/// LineTerminator 集合的并集）。
///
/// 与 Rust `char::is_whitespace` 的差异：规范集合不含 U+0085（NEL），手工
/// 按白名单匹配避免误剥。
fn is_js_ws(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000B}' | '\u{000C}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
                | '\u{000A}'
                | '\u{000D}'
                | '\u{2028}'
                | '\u{2029}'
    )
}

/// ToInt32：把 f64 转成 mod 2^32 回绕的有符号 i32。
///
/// 输入值 NaN、±0、±∞ 一律得 0；其余先向零截断，再对 2^32 取欧几里得
/// 余（余数恒非负），故超大输入（如 2^40）按回绕而非饱和处理。
fn to_int32(n: f64) -> i32 {
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return 0;
    }
    (n.trunc().rem_euclid(4294967296.0) as u32) as i32
}

/// `parseInt(string, radix)`：按指定进制解析字符串，取最长有效数字前缀。
///
/// 先用规范空白白名单修剪首尾，再读首个 `+`/`-` 符号。radix 先经 ToInt32
/// 转换（mod 2^32 回绕），NaN 与 undefined 均得 0；转换后 R 非零且越出
/// [2,36] 时整体返回 NaN。仅当转换后 R 为 0 或 16 时剥除 `0x`/`0X` 前缀
/// 并改按十六进制解析；随后按确定的进制收集最长连续有效数字前缀并转为
/// f64（十进制走正确舍入，其余进制数学累加，均允许超 2^53 的舍入），无
/// 任何有效数字时返回 NaN；结果落在 i32 域内用 int 表示，`-0` 保留负零。
pub fn number_parse_int<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // 参数按 ToString 完整转换：对象经 ToPrimitive(string hint)，Symbol 抛
    // TypeError；对象方法抛出的原生异常原样传播。
    let s = match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    let s = s.trim_start_matches(is_js_ws).trim_end_matches(is_js_ws);

    // radix 经 ToInt32：缺省参数视为 undefined（ToInt32 → 0）。
    let raw = if args.len() > 2 {
        match vm.coerce_number_bounded(vm.reg(args[2])) {
            Ok(n) => n,
            Err(_) => {
                // coercion 触发用户 valueOf/toString 抛出的异常经
                // last_uncaught_value 恢复后原样重新抛出。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert radix to a number"));
            }
        }
    } else {
        f64::NAN
    };
    let r = to_int32(raw);

    // 读符号并跳过；R≠0 且越出 [2,36] 直接 NaN（保留 0 参与后续 0x 判定）。
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let radix_default: u32 = if r == 0 {
        10
    } else if (2..=36).contains(&r) {
        r as u32
    } else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };

    // 0x/0X 前缀：仅当原 R 为 0（缺省/undefined/NaN/0）或 16 时剥前缀转十六进制。
    let (digits, radix) = if (r == 0 || r == 16)
        && rest.len() >= 2
        && rest.as_bytes()[0] == b'0'
        && (rest.as_bytes()[1] == b'x' || rest.as_bytes()[1] == b'X')
    {
        (&rest[2..], 16u32)
    } else {
        (rest, radix_default)
    };

    // 按进制收集最长连续有效数字前缀；十进制交 Rust 正确舍入解析
    // （逐位 f64 累加对 20+ 位数字会产生 1 ulp 级偏差），其余进制
    // 数学累加（2 的幂进制在 53 位内精确，超出按规范允许近似舍入）。
    let mut end = 0usize;
    for (i, c) in digits.char_indices() {
        if c.to_digit(radix).is_none() {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let prefix = &digits[..end];
    let acc = if radix == 10 {
        match prefix.parse::<f64>() {
            Ok(v) => v,
            Err(_) => return NativeResult::Ok(JsValue::float(f64::NAN)),
        }
    } else {
        let mut acc = 0.0f64;
        for c in prefix.chars() {
            acc = acc * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64;
        }
        acc
    };

    let acc = if neg { -acc } else { acc };
    if acc == 0.0 {
        // 负零保留符号（parseInt("-0") → -0）。
        return NativeResult::Ok(JsValue::float(if neg { -0.0 } else { 0.0 }));
    }
    if acc.fract() == 0.0 && acc >= i32::MIN as f64 && acc <= i32::MAX as f64 {
        NativeResult::Ok(JsValue::int(acc as i32))
    } else {
        NativeResult::Ok(JsValue::float(acc))
    }
}

/// `parseFloat(string)`：解析字符串前缀为浮点数。
///
/// 规范白名单 trim 后读 `+`/`-` 符号，特判精确大小写的 `Infinity`；随后按
/// StrDecimalLiteral 文法扫描最长合法十进制前缀（整数 + 可选小数 + 可选
/// 指数，`0x10` 在 'x' 处停止得 0，`1.2.3` 得 1.2），前缀子串交给
/// fast_float 解析（溢出归 ±Infinity），无合法前缀返回 NaN。
pub fn number_parse_float<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // 参数按 ToString 完整转换：对象经 ToPrimitive(string hint)，Symbol 抛
    // TypeError；对象方法抛出的原生异常原样传播。
    let s = match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    let s = s.trim_start_matches(is_js_ws).trim_end_matches(is_js_ws);

    // 读符号；`Infinity` 大小写敏感，其后可带任意后缀（取最长合法前缀）。
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest.starts_with("Infinity") {
        return NativeResult::Ok(JsValue::float(if neg { f64::NEG_INFINITY } else { f64::INFINITY }));
    }

    // 扫描 mantissa：数字与至多一个 '.'（'.' 后必须有数字，整数部分可为空）。
    let b = rest.as_bytes();
    let mut i = 0usize;
    let mut dot = false;
    let mut mantissa_digits = 0usize;
    let mut end = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            mantissa_digits += 1;
            end = i + 1;
            i += 1;
        } else if c == b'.' && !dot {
            dot = true;
            i += 1;
        } else {
            break;
        }
    }
    // 指数部分：e/E [+/-] 数字，且 mantissa 必须先有数字；指数无数字则不含 'e'。
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') && mantissa_digits > 0 {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            end = j;
        }
    }
    if mantissa_digits == 0 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    match fast_float::parse::<f64, _>(&rest[..end]) {
        Ok(v) => NativeResult::Ok(JsValue::float(if neg { -v } else { v })),
        Err(_) => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

/// thisNumberValue（spec 21.7.3 共用步 1）：Number 原始值直返；带
/// [[NumberData]] 的 Number 对象取被包值；其余一律 TypeError。
/// toFixed/toExponential/toPrecision/toString 四方法均以本助手起步，
/// 取代 ToObject 语义的 coerce（后者对 {} 静默装箱，不抛错）。
fn this_number_value<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<f64, JsValue> {
    if this_val.is_int() || this_val.is_double() {
        return Ok(oxide_runtime_api::to_number(this_val));
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            // Number.prototype 本身即 [[NumberData]] = +0 的 Number 对象；
            // 与 number_value_of 的 proto 特判同形。
            let number_proto = vm.session().builtin_world().number_proto.as_ptr() as *mut oxide_types::object::JsObject;
            if std::ptr::eq(ptr, number_proto) {
                return Ok(0.0);
            }
            let obj = unsafe { &*ptr };
            if obj.is_number_obj() {
                return Ok(oxide_runtime_api::to_number(obj.boxed_value()));
            }
        }
    }
    Err(crate::error::create_type_error(vm, "Cannot convert this value to a number"))
}

/// ToIntegerOrInfinity 数值核（前置完整 ToNumber 后调用）：NaN/±0 归 0，
/// ±∞ 取原值，其余向零截断。
fn to_integer_or_infinity(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc()
    }
}

/// f/p/radix 参数转换：完整 ToNumber（对象执行 ToPrimitive 且 valueOf/toString
/// 抛出的异常原样传播）后走 ToIntegerOrInfinity。纯核 `to_integer_or_infinity`
/// 不传播异常，不得用于参数位。
fn coerce_to_integer_or_infinity<H: VmHost>(vm: &mut H, arg: JsValue) -> Result<f64, JsValue> {
    let raw = match vm.coerce_number_bounded(arg) {
        Ok(n) => n,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, "Cannot convert argument to a number"));
        }
    };
    Ok(to_integer_or_infinity(raw))
}

// ── 精确十进制展开与 half-up 舍入 ──
// toFixed/toExponential/toPrecision 共享：f64 是二进有理数，精确十进制展开
// 有限；在 digit 串上按 keep 位 half-up 舍入（tie 取较大者，规范语义）并
// 传播进位。Rust `format!("{:.N$}")` 是 half-even，tie 面与规范分歧，
// 仅 String() 路径可用。

/// base 2^32 小整数（limb 升序），承载 f64 展开所需位宽（≤ 2^1127）。
#[derive(Clone)]
struct Dec(Vec<u32>);

impl Dec {
    fn from_u64(v: u64) -> Self {
        Dec(vec![v as u32, (v >> 32) as u32])
    }

    fn trim(&mut self) {
        while self.0.len() > 1 && *self.0.last().unwrap() == 0 {
            self.0.pop();
        }
    }

    fn is_zero(&self) -> bool {
        self.0.iter().all(|&l| l == 0)
    }

    /// 左移 k 位（× 2^k）：原位高→低；word j 的低段来自 limb j−ws，
    /// 高段来自 limb j−ws−1（处理 j 时该 limb 尚未被覆写）。
    fn shl(&mut self, k: u32) {
        let ws = (k / 32) as usize;
        let bs = k % 32;
        let n = self.0.len();
        let orig = self.0[..n].to_vec();
        self.0 = vec![0; n + ws + 1];
        let mut top = 0u32;
        for i in (0..n).rev() {
            let v = orig[i];
            if i == n - 1 && bs != 0 {
                top = v >> (32 - bs);
            }
            let low = if bs == 0 { v } else { v << bs };
            let carry_in = if i == 0 || bs == 0 { 0 } else { orig[i - 1] >> (32 - bs) };
            self.0[i + ws] = low | carry_in;
        }
        self.0[n + ws] = top;
        self.trim();
    }

    /// 右移 q 位（截断），q 超位宽时归 0。
    fn shr(&mut self, q: u32) {
        let ws = (q / 32) as usize;
        if ws >= self.0.len() {
            self.0 = vec![0];
            return;
        }
        self.0.drain(..ws);
        let bs = q % 32;
        if bs != 0 {
            let mut borrow = 0u32;
            for limb in self.0.iter_mut() {
                let cur = *limb;
                *limb = (cur >> bs) | borrow;
                borrow = cur << (32 - bs);
            }
        }
        self.trim();
    }

    /// 保留低 q 位（& (2^q − 1)）。
    fn rem_pow2(&mut self, q: u32) {
        let total = (self.0.len() * 32) as u32;
        if q >= total {
            return;
        }
        let wm = (q / 32) as usize;
        let bm = q % 32;
        if bm != 0 {
            self.0[wm] &= (1u32 << bm) - 1;
        }
        self.0.truncate(wm + 1);
        self.trim();
    }

    /// 乘单个小因子（digit 步进用 ×10）。
    fn mul_small(&mut self, k: u32) {
        let mut carry = 0u64;
        for limb in self.0.iter_mut() {
            let acc = (*limb as u64) * k as u64 + carry;
            *limb = acc as u32;
            carry = acc >> 32;
        }
        if carry != 0 {
            self.0.push(carry as u32);
        }
    }

    /// 就地整除 10，返回余数（一个十进制 digit）。
    fn divmod10(&mut self) -> u8 {
        let mut rem = 0u64;
        for i in (0..self.0.len()).rev() {
            let cur = (rem << 32) | self.0[i] as u64;
            self.0[i] = (cur / 10) as u32;
            rem = cur % 10;
        }
        self.trim();
        rem as u8
    }
}

/// f64 精确十进制展开：前 count 个有效数字与首位十进制指数。
///
/// 返回 (digits, e)：x = 0.d1d2… × 10^(e+1)，d1 ≠ 0；展开恰在 count 前
/// 终止时位数不足，缺失位恒 0（调用方按 0 补齐）。
fn significant_digits(x: f64, count: usize) -> (Vec<u8>, i32) {
    debug_assert!(x > 0.0 && x.is_finite() && count > 0);
    let b = x.to_bits();
    let (m, p) = if (b >> 52) == 0 {
        // 次正规：值 = frac × 2^-1074（隐含位 0）。
        (b as u64, -1074)
    } else {
        // 正规：显式隐含位补齐尾数（指数位先掩掉，勿随位模式带入）。
        (((b as u64 & ((1u64 << 52) - 1)) | (1u64 << 52)), ((b >> 52) as i32) - 1075)
    };
    if p >= 0 {
        // 整数面：V = M × 2^p，位数 D 定首位指数，弃低位后逐位提取。
        let mut v = Dec::from_u64(m);
        v.shl(p as u32);
        let mut d = 0usize;
        let mut tmp = v.clone();
        while !tmp.is_zero() {
            tmp.divmod10();
            d += 1;
        }
        let e = d as i32 - 1;
        let take = count.min(d);
        for _ in 0..(d - take) {
            v.divmod10();
        }
        let mut digits = Vec::with_capacity(take);
        for _ in 0..take {
            digits.push(v.divmod10());
        }
        digits.reverse();
        (digits, e)
    } else {
        // 分数面：整数部分 + 小数余数逐步 ×10 取商（前导零定 e）。
        let q = (-p) as u32;
        let (int_part, mut r) = if q < 64 {
            (m >> q, Dec::from_u64(m & ((1u64 << q) - 1)))
        } else {
            (0u64, Dec::from_u64(m))
        };
        let mut digits: Vec<u8> = Vec::new();
        let mut e: Option<i32> = None;
        if int_part != 0 {
            let mut ip = int_part;
            let mut id = Vec::new();
            while ip > 0 {
                id.push((ip % 10) as u8);
                ip /= 10;
            }
            id.reverse();
            e = Some(id.len() as i32 - 1);
            digits.extend(id);
        }
        let mut pos = 0usize;
        while digits.len() < count && !r.is_zero() {
            r.mul_small(10);
            let mut t = r.clone();
            t.shr(q);
            let d = t.0.first().copied().unwrap_or(0) as u8;
            r.rem_pow2(q);
            pos += 1;
            // 无前导整数时，首个非零位之前的零不显著（定 e）；
            // 有整数部分后每一位（含零）均显著。
            if d == 0 && e.is_none() {
                continue;
            }
            if e.is_none() {
                e = Some(-(pos as i32));
            }
            digits.push(d);
        }
        (digits, e.unwrap_or(0))
    }
}

/// 精确 digit 串 half-up 舍入：rd 为舍入判定位下标（负下标按 0），
/// 进位自最后一个渲染位 rd−1 向首位传播；渲染位之后的输入位原样保留
/// （不再被读取）。
///
/// 判定位 ≥ 5 进位（规范 tie 取较大 n：恰好相等也取较大者）；全链进位时
/// 值恰为 10^(e+1)，低位全 0、e + 1（9.99→10.0 型）。位数不足时缺失位
/// 恒 0（展开已终止），不进位。
fn round_digits(d: &[u8], e: i32, rd: i32) -> (Vec<u8>, i32) {
    // 舍入位在展开之前（rd < 0）时无渲染位，输出空串。
    let mut out = if rd < 0 { Vec::new() } else { d.to_vec() };
    let rd_digit = if rd >= 0 { d.get(rd as usize).copied().unwrap_or(0) } else { 0 };
    if rd_digit < 5 {
        return (out, e);
    }
    let mut k = rd - 1;
    loop {
        if k < 0 {
            // 无渲染位可进：0.x 型直接 10^e'。
            return (vec![1], e + 1);
        }
        out[k as usize] += 1;
        if out[k as usize] < 10 {
            return (out, e);
        }
        out[k as usize] = 0;
        if k == 0 {
            // 全链进位：值恰为 10^(e+1)，低位全 0。
            return (vec![1], e + 1);
        }
        k -= 1;
    }
}

/// digit i 落十进制位置 e−i；越界（含负下标）按 0。
fn dig(d: &[u8], i: i32) -> u8 {
    if i >= 0 {
        d.get(i as usize).copied().unwrap_or(0)
    } else {
        0
    }
}

/// 定点形渲染：整数部分自位置 e 至 0，小数部分 f 位，越界位按 0。
fn fixed_string(d: &[u8], e: i32, f: usize, neg: bool) -> String {
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if e < 0 {
        s.push('0');
    } else {
        // 整数部分自高位到低位：digit i 落位置 e−i。
        for i in 0..=e {
            s.push((b'0' + dig(d, i)) as char);
        }
    }
    if f > 0 {
        s.push('.');
        for p in 1..=f as i32 {
            s.push((b'0' + dig(d, e + p)) as char);
        }
    }
    s
}

/// 科学式渲染：d1[.d2…d(f+1)] 恒带符号指数。
fn exp_string(d: &[u8], e: i32, f: usize, neg: bool) -> String {
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push((b'0' + dig(d, 0)) as char);
    if f > 0 {
        s.push('.');
        for i in 1..=f as i32 {
            s.push((b'0' + dig(d, i)) as char);
        }
    }
    s.push('e');
    if e < 0 {
        s.push('-');
    } else {
        s.push('+');
    }
    s.push_str(&e.abs().to_string());
    s
}

/// `Number.prototype.toLocaleString()`：locale 参数忽略，恒按十进制输出。
/// 与 toString 的 radix-10 路径同形；不读第二个实参（length 为 0）。
pub fn number_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = match this_number_value(vm, vm.reg(args[0])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)))
}

/// `Number.prototype.toString(radix)`：按指定进制（2..36）转字符串。
///
/// 步 1 先 thisNumberValue（非 Number 原始值/对象抛 TypeError）；十进制走共享的
/// ECMA-262 Number::toString 格式化；非十进制对截断后的整数部分做进制转换
/// （小数部分按近似处理）。radix 经 ToInteger 后越界抛 RangeError，
/// NaN/Infinity 输出专名。
pub fn number_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = match this_number_value(vm, vm.reg(args[0])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    let radix = if args.len() > 1 {
        let radix_arg = vm.reg(args[1]);
        if radix_arg.is_undefined() {
            10u32
        } else {
            // radix 走 ToIntegerOrInfinity：先经对象 coercion（poisoned valueOf
            // 需传播其异常），NaN/±0 归 0 后越界抛 RangeError。
            let raw = match vm.coerce_number_bounded(radix_arg) {
                Ok(n) => n,
                Err(_) => {
                    // coercion 触发用户 valueOf/toString 抛出的异常经
                    // last_uncaught_value 恢复后原样重新抛出。
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert radix to a number"));
                }
            };
            let r = to_integer_or_infinity(raw);
            if !(2.0..=36.0).contains(&r) {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "toString() radix must be between 2 and 36",
                ));
            }
            r as u32
        }
    } else {
        10u32
    };

    if radix == 10 {
        return NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)));
    }

    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    if n.abs() >= u128::MAX as f64 {
        // 超出 u128 可精确表示的整数范围，退化为十进制近似。
        return NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)));
    }
    let neg = n.is_sign_negative();
    let mut value = n.abs().trunc() as u128;
    let mut result = String::new();
    if value == 0 {
        result.push('0');
    } else {
        let chars = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut digits = Vec::new();
        while value > 0 {
            digits.push(chars[(value % radix as u128) as usize] as char);
            value /= radix as u128;
        }
        for ch in digits.iter().rev() {
            result.push(*ch);
        }
    }
    if neg {
        result.insert(0, '-');
    }
    NativeResult::Ok(vm.new_string_owned(result))
}

/// `Number.prototype.toFixed(digits)`：固定小数位数（0..100）输出字符串，
/// 超出范围抛 RangeError；NaN/Infinity 输出专名。
///
/// 步序：先 thisNumberValue，再转换 fractionDigits 并做范围校验（范围检查
/// 先于 NaN 短路，NaN.toFixed(Infinity) 须抛 RangeError）；随后 x ≥ 10^21
/// 退化为 String(x) 科学式（±Infinity 同形，经此分支输出专名）；其余按精确
/// digit 串 half-up 舍入渲染（tie 取较大者，Rust 格式化 half-even 不可用）。
pub fn number_to_fixed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = match this_number_value(vm, vm.reg(args[0])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    let raw = if args.len() > 1 {
        match coerce_to_integer_or_infinity(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        0.0
    };
    if !(0.0..=100.0).contains(&raw) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toFixed() fractionDigits must be between 0 and 100",
        ));
    }
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    // x ≥ 10^21 用科学式 String 形态（±Infinity 同为该形态）。
    if n.abs() >= 1e21 {
        let s = if n < 0.0 {
            format!("-{}", oxide_runtime_api::js_number_to_string(-n))
        } else {
            oxide_runtime_api::js_number_to_string(n)
        };
        return NativeResult::Ok(vm.new_string_owned(s));
    }
    let f = raw as usize;
    let neg = n < 0.0;
    if n == 0.0 {
        // -0 落正分支：定点形无符号位可携。
        return NativeResult::Ok(vm.new_string_owned(fixed_string(&[], 0, f, false)));
    }
    // 定点形覆盖位置 e..−f；进位传播最多使 e+1，覆盖到舍入位共 e+f+2 位。
    // 舍入判定位 = 位置 −(f+1) 的 digit（e+f+1，负下标按 0）。
    let (_, e0) = significant_digits(n.abs(), 1);
    let count = (e0 + f as i32 + 2).max(1) as usize;
    let (d, e) = significant_digits(n.abs(), count);
    let (d, e) = round_digits(&d, e, e0 + f as i32 + 1);
    NativeResult::Ok(vm.new_string_owned(fixed_string(&d, e, f, neg)))
}

/// `Number.isInteger`：参数是有限且无小数部分的数值才返回 true。
pub fn number_is_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    NativeResult::Ok(JsValue::bool(n.trunc() == n && n.is_finite()))
}

/// `Number.isSafeInteger`：参数是安全整数范围（±2^53-1）内的整数才返回 true。
pub fn number_is_safe_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    let safe = n.trunc() == n && n.is_finite() && n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64;
    NativeResult::Ok(JsValue::bool(safe))
}

/// `Number.prototype.toPrecision(precision)`：按有效数字位数（1..100）输出，
/// 科学计数法与定点表示按指数自动切换，超范围抛 RangeError。
///
/// 步序：先 thisNumberValue；precision 缺省直接返 ToString(x)（含 NaN/±∞
/// 全形）；再转 precision，NaN/±∞ 专名先于范围校验（(±∞).toPrecision(1000)
/// 返 "Infinity" 而非 RangeError）；最后按精确 digit 串 half-up 舍入，
/// 定点/指数分界用**进位后**的首位指数（999.toPrecision(2) = "1.0e+3"）。
pub fn number_to_precision<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = match this_number_value(vm, vm.reg(args[0])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    if args.len() <= 1 || vm.reg(args[1]).is_undefined() {
        return NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)));
    }
    let raw = match coerce_to_integer_or_infinity(vm, vm.reg(args[1])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    if !(1.0..=100.0).contains(&raw) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toPrecision() precision must be between 1 and 100",
        ));
    }
    let precision = raw as usize;
    let neg = n < 0.0;
    if n == 0.0 {
        // -0 落正分支；"0" 后跟 precision−1 个零。
        let mut s = String::from("0");
        if precision > 1 {
            s.push('.');
            for _ in 0..precision - 1 {
                s.push('0');
            }
        }
        return NativeResult::Ok(vm.new_string_owned(s));
    }
    let (d, e) = significant_digits(n.abs(), precision + 1);
    let (d, e) = round_digits(&d, e, precision as i32);
    let s = if e >= -6 && e < precision as i32 {
        fixed_string(&d, e, (precision as i32 - e - 1) as usize, neg)
    } else {
        exp_string(&d, e, precision - 1, neg)
    };
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Number.prototype.toExponential(digits)`：按科学计数法输出；
/// digits 缺省/undefined 取最短精确有效数字数，超范围抛 RangeError。
///
/// 步序：先 thisNumberValue，再转换 fractionDigits（对象参数执行 ToPrimitive
/// 且异常原样传播）；NaN/±∞ 专名先于范围检查；缺省位数走最短 round-trip
/// 前缀，显式位数按精确 digit 串 half-up 舍入（tie 取较大者）；指数恒带符号。
pub fn number_to_exponential<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = match this_number_value(vm, vm.reg(args[0])) {
        Ok(v) => v,
        Err(exc) => return NativeResult::Err(exc),
    };
    let raw = if args.len() > 1 && !vm.reg(args[1]).is_undefined() {
        match coerce_to_integer_or_infinity(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        f64::NAN
    };
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let has_digits = !raw.is_nan();
    if has_digits && !(0.0..=100.0).contains(&raw) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toExponential() fractionDigits must be between 0 and 100",
        ));
    }
    let neg = n < 0.0;
    if n == 0.0 {
        // -0 落正分支：科学式 "0e+0" 形。
        let f = if has_digits { raw as usize } else { 0 };
        return NativeResult::Ok(vm.new_string_owned(exp_string(&[], 0, f, false)));
    }
    if !has_digits {
        // 缺省位数 = Rust {:e} 最短 round-trip 形（规范"最短精确有效数字数"
        // 同口径），仅补指数符号。
        let mut s = format!("{:e}", n.abs());
        if let Some(e_pos) = s.find('e') {
            if !s[e_pos + 1..].starts_with('-') {
                s.insert(e_pos + 1, '+');
            }
        }
        let result = format!("{}{}", if neg { "-" } else { "" }, s);
        return NativeResult::Ok(vm.new_string_owned(result));
    }
    let f = raw as usize;
    let (d, e) = significant_digits(n.abs(), f + 2);
    let (d, e) = round_digits(&d, e, f as i32 + 1);
    NativeResult::Ok(vm.new_string_owned(exp_string(&d, e, f, neg)))
}

/// `Number.prototype.valueOf`：返回包装对象的原始 number；
/// 原始 number 直接返回，非 Number 对象抛 TypeError。
pub fn number_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_int() || this_val.is_double() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            // Number.prototype 本身是 Number 对象，其 [[NumberData]] 为 +0。
            let number_proto = vm.session().builtin_world().number_proto.as_ptr() as *mut oxide_types::object::JsObject;
            if ptr == number_proto {
                return NativeResult::Ok(JsValue::int(0));
            }
            let obj = unsafe { &*ptr };
            if obj.is_number_obj() {
                return NativeResult::Ok(obj.boxed_value());
            }
        }
    }
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "Number.prototype.valueOf called on incompatible receiver",
    ))
}
