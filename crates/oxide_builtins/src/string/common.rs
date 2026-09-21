use std::borrow::Cow;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::VmHost;

macro_rules! try_string {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    };
}
pub(crate) use try_string;

// ── 文本通道 ─────────────────────────────────────────────────────────────
//
// 字符串值有三种载荷形态：Flat（良形 UTF-8 字节串）、FlatU16（UTF-16 单元
// 数组，可含孤立 surrogate）、Cons（分段拼接的 rope）。正则匹配按载荷形态
// 分两条入口：Str 入口以字节串调用匹配器，返回的匹配范围以字节为单位；
// Units 入口以单元数组调用匹配器，匹配范围以 UTF-16 码元为单位。两条入口
// 的范围单位不可互换，匹配片段端点必须换算到与所在入口相同的单位；
// 正则之外的 String 方法统一取单元序列处理。

/// 正则家族的匹配文本通道。
pub(crate) enum MatchText<'a> {
    Str(&'a str),
    Units(Cow<'a, [u16]>),
}

impl<'a> MatchText<'a> {
    /// 从字符串值按载荷形态借用：Flat → Str 零拷贝，其余 → Units
    /// （FlatU16 直接借用、Cons 经扁平化缓存）。
    fn from_value(val: JsValue) -> Self {
        debug_assert!(val.is_string());
        // SAFETY: val 为字符串值，借用期存活由调用方借用纪律保证。
        let sp = unsafe { &*val.as_string_ptr() };
        if sp.is_flat() {
            MatchText::Str(sp.as_str())
        } else {
            MatchText::Units(sp.units())
        }
    }

    /// 码元数。
    pub(crate) fn len_units(&self) -> usize {
        match self {
            MatchText::Str(s) => {
                if s.is_ascii() {
                    s.len()
                } else {
                    s.encode_utf16().count()
                }
            }
            MatchText::Units(u) => u.len(),
        }
    }

    /// 借出单元序列（Str 臂一次性编码为 owned；Units 臂借用/浅拷）。
    pub(crate) fn units(&self) -> Cow<'a, [u16]> {
        match self {
            MatchText::Str(s) => {
                if s.is_ascii() {
                    Cow::Owned(s.as_bytes().iter().map(|&b| b as u16).collect())
                } else {
                    Cow::Owned(s.encode_utf16().collect())
                }
            }
            MatchText::Units(u) => u.clone(),
        }
    }

    /// 从码元位置开始的首个匹配（Str 臂内部换算为字节位置）。
    pub(crate) fn find_from_units(&self, regex: &regress::Regex, start_units: usize) -> Option<regress::Match> {
        match self {
            MatchText::Str(s) => regex.find_from(s, unit_to_byte(s, start_units)).next(),
            MatchText::Units(u) => regex.find_from_utf16(u, start_units).next(),
        }
    }

    /// 按序消费全部非重叠匹配（自码元 0 起）。
    pub(crate) fn for_each_match(&self, regex: &regress::Regex, mut f: impl FnMut(&regress::Match)) {
        match self {
            MatchText::Str(s) => {
                for m in regex.find_iter(s) {
                    f(&m);
                }
            }
            MatchText::Units(u) => {
                for m in regex.find_from_utf16(u, 0) {
                    f(&m);
                }
            }
        }
    }

    /// 匹配片段为单元序列（start/end 取臂自身匹配范围口径：Str 臂字节、
    /// Units 臂码元——臂由文本通道钉死，调用方直接传 `m.range()`）。
    pub(crate) fn slice(&self, start: usize, end: usize) -> Cow<'a, [u16]> {
        match self {
            MatchText::Str(s) => {
                let sub = &s[start..end];
                Cow::Owned(if sub.is_ascii() {
                    sub.as_bytes().iter().map(|&b| b as u16).collect()
                } else {
                    sub.encode_utf16().collect()
                })
            }
            MatchText::Units(u) => Cow::Owned(u[start..end].to_vec()),
        }
    }

    /// 匹配范围端点换算到码元口径（Str 臂范围是字节口径，Units 臂即码元）。
    pub(crate) fn unit_pos(&self, pos: usize) -> usize {
        match self {
            MatchText::Str(s) => byte_to_unit(s, pos),
            MatchText::Units(_) => pos,
        }
    }
}

/// 良形文本的前置转换缓存：owned 匹配文本（regexp 模块的 haystack 载体）。
pub(crate) enum OwnedText {
    Str(String),
    Units(Vec<u16>),
}

impl OwnedText {
    pub(crate) fn as_match_text(&self) -> MatchText<'_> {
        match self {
            OwnedText::Str(s) => MatchText::Str(s),
            OwnedText::Units(u) => MatchText::Units(Cow::Borrowed(u)),
        }
    }

    /// 构造字符串值：Str 变体按字节串、Units 变体按单元数组，载荷形态各自最小。
    pub(crate) fn to_value<H: VmHost>(&self, vm: &mut H) -> JsValue {
        match self {
            OwnedText::Str(s) => vm.new_string(s),
            OwnedText::Units(u) => vm.new_string_units_owned(u.clone()),
        }
    }
}

// ── 单元基础工具 ─────────────────────────────────────────────────────────

/// 单元序列首个匹配位置（自 from 起，返回 `hay` 内的绝对下标）；needle 空
/// 由调用方先行处理。
pub(crate) fn find_units(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    let hay = &hay[from..];
    if needle.len() == 1 {
        return hay.iter().position(|&u| u == needle[0]).map(|i| from + i);
    }
    hay.windows(needle.len()).position(|w| w == needle).map(|i| from + i)
}

/// 单元序列末个匹配位置（限 end 前缀内起始）。
pub(crate) fn rfind_units(hay: &[u16], needle: &[u16], end: usize) -> Option<usize> {
    let hay = &hay[..end];
    if needle.len() == 1 {
        return hay.iter().rposition(|&u| u == needle[0]);
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// 自 start 起的最长良形段终点（代理对整体归段，孤立 surrogate 截断段）。
fn well_formed_segment_end(units: &[u16], start: usize) -> usize {
    let mut i = start;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u) {
            if i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
                i += 2;
            } else {
                break;
            }
        } else if (0xDC00..=0xDFFF).contains(&u) {
            break;
        } else {
            i += 1;
        }
    }
    i
}

/// 码点数（代理对计 1，孤立 surrogate 计 1）——pad 族的目标长度口径。
pub(crate) fn code_point_count(units: &[u16]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < units.len() {
        if (0xD800..=0xDBFF).contains(&units[i]) && i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
            i += 2;
        } else {
            i += 1;
        }
        n += 1;
    }
    n
}

/// 取序列头 count 个码点对应的单元前缀。
pub(crate) fn take_code_points(units: &[u16], count: usize) -> &[u16] {
    let mut n = 0;
    let mut i = 0;
    while i < units.len() && n < count {
        if (0xD800..=0xDBFF).contains(&units[i]) && i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
            i += 2;
        } else {
            i += 1;
        }
        n += 1;
    }
    &units[..i]
}

/// ES 白空格判定的单元口径：Rust 空白表与 White_Space+LineTerminator 等价；
/// 孤立 surrogate 非白空格。
pub(crate) fn is_trim_unit(u: u16) -> bool {
    if u < 0x80 {
        return matches!(u, 0x09..=0x0D | 0x20);
    }
    if let Some(c) = char::from_u32(u as u32) {
        return c.is_whitespace();
    }
    false
}

/// 良形文本的字节偏移 → 码元数（Str 臂匹配范围/index 的码元换算入口）。
pub(crate) fn byte_to_unit(s: &str, byte: usize) -> usize {
    if s.is_ascii() {
        byte
    } else {
        s[..byte].encode_utf16().count()
    }
}

/// 良形文本的码元位置 → 字节偏移（位置须落在字符边界：匹配游标恒对齐）。
pub(crate) fn unit_to_byte(s: &str, unit: usize) -> usize {
    if s.is_ascii() {
        return unit.min(s.len());
    }
    let mut acc = 0usize;
    for (byte, ch) in s.char_indices() {
        if unit == acc {
            return byte;
        }
        acc += ch.len_utf16();
    }
    s.len()
}

/// 大小写/规范化按"良形段"分段映射：整段经 UTF-8 往返调用映射函数，孤立
/// surrogate 单元原样透传（映射对码点定义，对孤立 surrogate 无定义）。
pub(crate) fn map_well_formed_segments(units: &[u16], mut f: impl FnMut(&str) -> String) -> Vec<u16> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let seg_start = i;
        i = well_formed_segment_end(units, i);
        if i == seg_start {
            out.push(units[i]);
            i += 1;
            continue;
        }
        let s = String::from_utf16(&units[seg_start..i]).expect("良形段 from_utf16 必成功");
        out.extend(f(&s).encode_utf16());
    }
    out
}

// ── this / 参数转换 ─────────────────────────────────────────────────────

/// 取 this 的单元序列供只读扫描：原始字符串按载荷形态借用（Flat 惰性编码、
/// FlatU16 直接借用、Cons 经扁平化缓存），对象经完整 ToString。
/// null/undefined/symbol 抛 TypeError；对象 ToString 抛出的原生异常原样传播。
///
/// # 注意事项
/// 返回借用绑定本次 `&mut` 借用，存活期内禁止任何 `&mut` 调用；需与参数并存
/// 时调用方先 `into_owned` 落地。
pub(crate) fn this_units<'a, H: VmHost>(vm: &'a mut H, args: &[u8]) -> Result<Cow<'a, [u16]>, JsValue> {
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        return Err(crate::error::create_type_error(vm, "String.prototype method called on null or undefined"));
    }
    if this_val.is_symbol() {
        return Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }
    if this_val.is_string() {
        return Ok(vm.string_units(this_val));
    }
    match oxide_runtime_api::to_string_value_full(this_val, vm) {
        Ok(v) => Ok(vm.string_units(v)),
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播，
            // 否则会被展平为普通 Error 丢失原始异常对象。
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert this value to a string"))
        }
    }
}

/// 取 this 的匹配文本通道：原始字符串按载荷形态零拷贝借用（Flat → Str、其余
/// → Units），对象经完整 ToString（结果为字符串值，同形态借用）。
/// null/undefined/symbol 抛 TypeError；对象 ToString 抛出的原生异常原样传播。
pub(crate) fn this_text<'a, H: VmHost>(vm: &'a mut H, args: &[u8]) -> Result<MatchText<'a>, JsValue> {
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        return Err(crate::error::create_type_error(vm, "String.prototype method called on null or undefined"));
    }
    if this_val.is_symbol() {
        return Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }
    if this_val.is_string() {
        return Ok(MatchText::from_value(this_val));
    }
    match oxide_runtime_api::to_string_value_full(this_val, vm) {
        Ok(v) => Ok(MatchText::from_value(v)),
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert this value to a string"))
        }
    }
}

/// 取参数字符串的单元序列：原始字符串按载荷形态借用，其余经完整 ToString。
/// Symbol 与其余转换失败按规范抛 TypeError；对象 ToString 抛出的原生异常原样传播。
pub(crate) fn as_units<'a, H: VmHost>(vm: &'a mut H, val: JsValue) -> Result<Cow<'a, [u16]>, JsValue> {
    if val.is_string() {
        return Ok(vm.string_units(val));
    }
    match oxide_runtime_api::to_string_value_full(val, vm) {
        Ok(v) => Ok(vm.string_units(v)),
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"))
        }
    }
}

// ── 数组产出 ─────────────────────────────────────────────────────────────

/// 以已构造的字符串值构建字符串数组（元素零拷贝落地），供逐单元产出路径
/// （空分隔 split）复用，跳过 `Vec<String>` 中间层。
pub(crate) fn make_string_array_values<H: VmHost>(vm: &mut H, parts: Vec<JsValue>) -> JsValue {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let n = parts.len();
    let arr =
        vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto), n, vm.epoch().bump()));
    unsafe {
        for (i, sv) in parts.into_iter().enumerate() {
            (*arr).set_prop_at(i, sv);
        }
        (*arr).set_prop_count(n);
    }
    JsValue::from_js_object(arr)
}

/// 以单元序列集合构建字符串数组（元素按智能路由物化为字符串值），供
/// split/match 的片段产出路径复用。
pub(crate) fn make_units_array<H: VmHost>(vm: &mut H, parts: Vec<Vec<u16>>) -> JsValue {
    let values: Vec<JsValue> = parts.into_iter().map(|u| vm.new_string_units_owned(u)).collect();
    make_string_array_values(vm, values)
}
