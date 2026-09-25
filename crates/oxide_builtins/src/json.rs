use std::collections::HashSet;
use std::fmt::Write;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{int_key_value, is_int_key, make_int_key};
use oxide_types::value::JsValue;

use crate::object::walk_own_keys;

use oxide_runtime_api::{NativeResult, VmHost};

/// `JSON.parse(text, reviver)`：解析 JSON 文本为 JS 值（递归下降解析器，
/// 逐原始值记录精确源文本切片）。提供 reviver 时以后序遍历逐属性调用
/// reviver 重建值，第三参 context 对象携带 `source` 属性。
///
/// `text` 经 `? ToString` 强转：对象走 ToPrimitive（string hint），Symbol
/// 抛 TypeError，强转期用户异常原值传播。
pub fn json_parse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "JSON.parse requires 1 argument"));
    }
    let text_val = match oxide_runtime_api::to_string_value_full(vm.reg(args[1]), vm) {
        Ok(v) => v,
        Err(_) => {
            // 强转期用户异常原值重抛；Symbol 无在途异常，按规范构造 TypeError。
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
        }
    };
    let text = {
        // SAFETY: text_val 已确认是字符串值。
        unsafe { (*text_val.as_string_ptr()).to_owned_string() }
    };

    let parsed = match parse_json(&text) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_syntax_error(vm, &msg)),
    };

    let mut result = build_js_value(vm, &parsed);

    // reviver 遍历以 holder 包装对象为根：解析得到的根值存于空串键槽，
    // 后序遍历自该槽展开。返回值 = 根级 reviver 返回值原样（wrapper 的
    // `''` 槽由父级循环施加，根级无父级，槽恒不被触碰）。
    if args.len() > 2 {
        let reviver_val = vm.reg(args[2]);
        if reviver_val.is_object() {
            let rptr = reviver_val.as_js_object_ptr();
            if !rptr.is_null() && unsafe { (*rptr).is_function() } {
                let empty_si = vm.kernel_core().perm_interner().intern("").0;
                let holder = create_wrapper(vm, result);
                let holder_ptr = holder.as_js_object_ptr();
                result = match walk_reviver(vm, holder_ptr, empty_si, reviver_val, Some(&parsed)) {
                    Ok(new_val) => new_val,
                    Err(e) => return NativeResult::Err(e),
                };
            }
        }
    }

    NativeResult::Ok(result)
}

/// rawJSON 对象禁止的首/末 code unit 集：TAB/LF/CR/SPACE。
const RAW_JSON_FORBIDDEN_EDGE_UNITS: [u16; 4] = [0x09, 0x0A, 0x0D, 0x20];

/// `JSON.rawJSON(text)`：把合法 JSON 文本包装为 raw JSON 对象。
///
/// # 步骤
/// 1. `? ToString(text)`（Symbol 抛 TypeError，对象经 ToPrimitive，用户异常原值传播）。
/// 2. 空串或首/末 code unit 为 TAB/LF/CR/SPACE 抛 SyntaxError。
/// 3. 须为合法 JSON 文本且最外层非 object/array，否则 SyntaxError。
/// 4. 建 null 原型 frozen 对象，唯一自身属性 `rawJSON` = 文本
///    （writable:false、enumerable:true、configurable:false）。
///
/// # 副作用
/// - 分配一个 session/epoch 对象与其字符串属性值。
pub fn json_raw_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let text_val = match oxide_runtime_api::to_string_value_full(
        if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() },
        vm,
    ) {
        Ok(v) => v,
        Err(_) => {
            // 强转期用户异常原值重抛；Symbol 无在途异常，按规范构造 TypeError。
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
        }
    };
    let text = {
        // SAFETY: text_val 已确认是字符串值。
        unsafe { (*text_val.as_string_ptr()).to_owned_string() }
    };

    // 边界字符校验：空串或首/末 code unit 属空白四元之一。
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty()
        || RAW_JSON_FORBIDDEN_EDGE_UNITS.contains(&units[0])
        || RAW_JSON_FORBIDDEN_EDGE_UNITS.contains(&units[units.len() - 1])
    {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "Invalid JSON text"));
    }

    // 合法性校验：解析失败即非合法 JSON 文本；最外层
    // object/array 不满足 raw JSON 语义。
    match parse_json(&text) {
        Ok(JsonNode {
            kind: JsonKind::Object(_) | JsonKind::Array(_),
            ..
        }) => {
            return NativeResult::Err(crate::error::create_syntax_error(vm, "Invalid JSON text"));
        }
        Ok(_) => {}
        Err(msg) => return NativeResult::Err(crate::error::create_syntax_error(vm, &msg)),
    }

    // 建 null 原型对象：rawJSON 属性为唯一自身属性，frozen 形态收尾。
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.type_tag = JsObject::OBJ_TYPE_RAW_JSON;
    let raw_si = vm.kernel_core().perm_interner().intern("rawJSON").0;
    let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), raw_si);
    obj.set_shape_id(new_shape);
    obj.ensure_hash_props().push(text_val);
    let obj_ptr = vm.alloc_object(obj);
    {
        let obj = unsafe { &mut *obj_ptr };
        obj.set_data_meta(0, PropAttributes::new(false, true, false));
        obj.set_frozen(true);
        obj.set_extensible(false);
    }
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
}

/// `JSON.isRawJSON(value)`：value 带 raw JSON 类型标签时返回 true，其余一律 false。
pub fn json_is_raw_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let is_raw = val.is_object() && {
        let p = val.as_js_object_ptr();
        !p.is_null() && unsafe { (*p).is_raw_json_obj() }
    };
    NativeResult::Ok(JsValue::bool(is_raw))
}

/// 解析后的 JSON 节点：值 + 精确源文本切片（仅原始值携带）。
/// 切片供 reviver context 的 `source` 属性与 SameValue-as 判定使用。
enum JsonKind {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonNode>),
    Object(Vec<(String, JsonNode)>),
}

struct JsonNode {
    kind: JsonKind,
    /// 精确源文本切片（数字不规范化、字符串含引号）；array/object 为 `None`。
    source: Option<String>,
}

/// 递归下降 JSON 解析器：接受 JSON 文本语法（空白限 TAB/LF/CR/SPACE），
/// 逐原始值记录精确源文本切片。
///
/// # 边界与前提
/// - 前导零数字、尾随逗号、溢出（1e400）、孤立 surrogate 均报 SyntaxError。
/// - 嵌套 object/array 深度上限 `MAX_JSON_DEPTH` 层，超限报 SyntaxError
///   （进入时先计入再判定，防深嵌套在递归展开前耗尽栈）。
/// - 重复对象键：末写胜（键序由 build_js_value 统一）。
fn parse_json(text: &str) -> Result<JsonNode, String> {
    let mut p = JsonParser {
        text,
        pos: 0,
        depth: 0,
    };
    p.skip_ws();
    let node = p.parse_value()?;
    p.skip_ws();
    if p.pos != text.len() {
        return Err("trailing characters".into());
    }
    Ok(node)
}

/// 嵌套容器深度上限：object/array 进入时计入，超限报 SyntaxError。
const MAX_JSON_DEPTH: usize = 128;

struct JsonParser<'a> {
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.text.len() {
            match self.text.as_bytes()[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.text.get(self.pos..).and_then(|s| s.chars().next())
    }

    /// 以当前游标为终点物化原始值节点（源切片 = `start..pos`）。
    fn prim(&self, kind: JsonKind, start: usize) -> JsonNode {
        JsonNode {
            kind,
            source: Some(self.text[start..self.pos].to_string()),
        }
    }

    fn expect_word(&mut self, word: &str) -> Result<(), String> {
        let end = self.pos + word.len();
        if self.text.get(self.pos..end) == Some(word) {
            self.pos = end;
            Ok(())
        } else {
            Err("invalid literal".into())
        }
    }

    fn parse_value(&mut self) -> Result<JsonNode, String> {
        self.skip_ws();
        let start = self.pos;
        let c = self.peek().ok_or_else(|| "unexpected end of input".to_string())?;
        match c {
            b'n' => {
                self.expect_word("null")?;
                Ok(self.prim(JsonKind::Null, start))
            }
            b't' => {
                self.expect_word("true")?;
                Ok(self.prim(JsonKind::Bool(true), start))
            }
            b'f' => {
                self.expect_word("false")?;
                Ok(self.prim(JsonKind::Bool(false), start))
            }
            b'"' => {
                let s = self.parse_string()?;
                Ok(self.prim(JsonKind::String(s), start))
            }
            b'[' => {
                // 深度计数：进入时先 +1 再判上限，超限在递归展开前拒绝。
                self.depth += 1;
                if self.depth > MAX_JSON_DEPTH {
                    self.depth -= 1;
                    return Err("recursion limit exceeded".into());
                }
                let items = self.parse_array()?;
                self.depth -= 1;
                Ok(JsonNode {
                    kind: JsonKind::Array(items),
                    source: None,
                })
            }
            b'{' => {
                self.depth += 1;
                if self.depth > MAX_JSON_DEPTH {
                    self.depth -= 1;
                    return Err("recursion limit exceeded".into());
                }
                let entries = self.parse_object()?;
                self.depth -= 1;
                Ok(JsonNode {
                    kind: JsonKind::Object(entries),
                    source: None,
                })
            }
            b'-' | b'0'..=b'9' => {
                let n = self.parse_number()?;
                Ok(self.prim(JsonKind::Number(n), start))
            }
            _ => Err("expected value".into()),
        }
    }

    fn parse_array(&mut self) -> Result<Vec<JsonNode>, String> {
        // 当前字节为 '['。
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            let item = self.parse_value()?;
            items.push(item);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    // 尾随逗号：',' 后必须跟值。
                    if self.peek() == Some(b']') {
                        return Err("trailing comma".into());
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(items);
                }
                _ => return Err("expected ',' or ']'".into()),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Vec<(String, JsonNode)>, String> {
        // 当前字节为 '{'。
        self.pos += 1;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(entries);
        }
        loop {
            self.skip_ws();
            let key = match self.peek() {
                Some(b'"') => self.parse_string()?,
                _ => return Err("expected property name".into()),
            };
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err("expected ':'".into());
            }
            self.pos += 1;
            let val = self.parse_value()?;
            entries.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    // 尾随逗号：',' 后必须跟键。
                    if self.peek() == Some(b'}') {
                        return Err("trailing comma".into());
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(entries);
                }
                _ => return Err("expected ',' or '}'".into()),
            }
        }
    }

    /// JSON 数字文法：`-?(0|[1-9]digits)(.digits)?([eE][+-]?digits)?`；
    /// 溢出（1e400）报范围错误，下溢（1e-400）归零。
    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err("invalid number".into()),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err("invalid number".into());
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if let Some(c) = self.peek() {
            if c == b'e' || c == b'E' {
                self.pos += 1;
                if let Some(s) = self.peek() {
                    if s == b'+' || s == b'-' {
                        self.pos += 1;
                    }
                }
                if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    return Err("invalid number".into());
                }
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        let slice = &self.text[start..self.pos];
        let n = slice.parse::<f64>().map_err(|_| "invalid number".to_string())?;
        if !n.is_finite() {
            return Err("number out of range".into());
        }
        Ok(n)
    }

    fn parse_string(&mut self) -> Result<String, String> {
        // 当前字节为开引号。
        self.pos += 1;
        let mut units: Vec<u16> = Vec::new();
        loop {
            let c = self.peek_char().ok_or_else(|| "unterminated string".to_string())?;
            match c {
                '"' => {
                    self.pos += 1;
                    // 解析期已拒绝孤立 surrogate，单元序列恒为良形 UTF-16。
                    return Ok(String::from_utf16(&units).expect("no lone surrogate survives parse"));
                }
                '\\' => {
                    self.pos += 1;
                    let e = self.peek_char().ok_or_else(|| "unterminated escape".to_string())?;
                    self.pos += 1;
                    let u = match e {
                        '"' => 0x22,
                        '\\' => 0x5C,
                        '/' => 0x2F,
                        'b' => 0x08,
                        'f' => 0x0C,
                        'n' => 0x0A,
                        'r' => 0x0D,
                        't' => 0x09,
                        'u' => self.parse_unicode_escape()?,
                        _ => return Err("invalid escape".into()),
                    };
                    units.push(u as u16);
                }
                c => {
                    if (c as u32) <= 0x1F {
                        return Err("unescaped control character in string".into());
                    }
                    // 码元编码（含 astral 字符双单元）。
                    let mut buf = [0u16; 2];
                    let written = c.encode_utf16(&mut buf);
                    units.extend_from_slice(written);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<u32, String> {
        let hi = self.parse_hex4()?;
        if (0xD800..=0xDBFF).contains(&hi) {
            // 代理对：高 surrogate 必须紧跟 \uXXXX 低 surrogate。
            if self.text.get(self.pos..).is_some_and(|s| s.starts_with("\\u")) {
                self.pos += 2;
                let lo = self.parse_hex4()?;
                if (0xDC00..=0xDFFF).contains(&lo) {
                    return Ok(0x10000 + ((u32::from(hi) - 0xD800) << 10) + (u32::from(lo) - 0xDC00));
                }
                return Err("invalid surrogate pair".into());
            }
            return Err("lone leading surrogate in hex escape".into());
        }
        if (0xDC00..=0xDFFF).contains(&hi) {
            return Err("lone leading surrogate in hex escape".into());
        }
        Ok(u32::from(hi))
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let mut v: u16 = 0;
        for _ in 0..4 {
            let c = self.peek_char().ok_or_else(|| "unexpected end of hex escape".to_string())?;
            let d = c
                .to_digit(16)
                .map(|d| d as u16)
                .ok_or_else(|| "invalid hex digit".to_string())?;
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(v)
    }
}

/// InternalizeJSONProperty 后序遍历：Get 读当前值 → 递归子节点并把每子返回值
/// 施加于容器（undefined → [[Delete]]，否则 CreateDataProperty，两失败路径
/// 静默）→ 构造 context 对象 → 调用 reviver。返回值 = 本级 reviver 返回值
/// 原样，由父级循环施加（根级由 json_parse 直接作 parse 结果）。
///
/// `node` 是本级对应的解析节点（可缺）：子集恒按当前值（容器可被 reviver
/// 前向改写，键集与解析节点不再重合）；节点仅作子节点映射与 context 的
/// `source` 属性来源，无节点处按空 context 处理。
fn walk_reviver<H: VmHost>(
    vm: &mut H, holder_ptr: *mut JsObject, key_si: u32, reviver: JsValue, node: Option<&JsonNode>,
) -> Result<JsValue, JsValue> {
    let holder_val = JsValue::from_js_object(holder_ptr);

    // 读臂：完整 Get（原型链 + 自身访问器触发），getter 抛出传播原值。
    let val = match vm.ordinary_get(unsafe { &*holder_ptr }, key_si, holder_val) {
        Ok(v) => v,
        Err(msg) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            return Err(exc);
        }
    };

    // 后序遍历：先递归子节点，每子返回值施加于容器。
    if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if !obj_ptr.is_null() {
            if unsafe { (*obj_ptr).is_array() } {
                let len = unsafe { (*obj_ptr).prop_count() } as usize;
                for i in 0..len {
                    let child_si = make_int_key(i as u32);
                    let child_node = node.and_then(|n| match &n.kind {
                        JsonKind::Array(items) => items.get(i),
                        _ => None,
                    });
                    let new_val = walk_reviver(vm, obj_ptr, child_si, reviver, child_node)?;
                    apply_child_result(vm, obj_ptr, child_si, new_val);
                }
            } else {
                // 对象臂：节点键经同一键规范化映射 si（重复键末写胜，
                // 与 build_js_value 一致），容器键逐一按 si 定位节点。
                let child_nodes = match node {
                    Some(JsonNode {
                        kind: JsonKind::Object(entries),
                        ..
                    }) => {
                        let mut seen: Vec<(u32, &JsonNode)> = Vec::new();
                        for (key, val) in entries {
                            let si = vm.string_key_si(key);
                            match seen.iter_mut().find(|(s, _)| *s == si) {
                                Some(slot) => slot.1 = val,
                                None => seen.push((si, val)),
                            }
                        }
                        seen
                    }
                    _ => Vec::new(),
                };
                let keys = {
                    let obj = unsafe { &*obj_ptr };
                    walk_own_keys(vm, obj)
                };
                for (child_si, _child_pos) in keys {
                    let child_node = child_nodes.iter().find(|(s, _)| *s == child_si).map(|(_, n)| *n);
                    let new_val = walk_reviver(vm, obj_ptr, child_si, reviver, child_node)?;
                    apply_child_result(vm, obj_ptr, child_si, new_val);
                }
            }
        }
    }

    // context 对象：仅当当前值与解析时值 SameValue-as 时携带 source。
    let context = build_context(vm, val, node);

    // 对当前值调用 reviver，返回值上抛由父级施加。
    let key_val = crate::object::key_si_to_js_value(vm, key_si);
    match vm.call_function_sync(reviver, holder_val, &[key_val, val, context]) {
        Ok(new_val) => Ok(new_val),
        Err(msg) => {
            // reviver 内抛出的原始值原样传播（不折叠为 TypeError 文本）。
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            Err(exc)
        }
    }
}

/// 构造 reviver 的 context 对象：普通对象（proto = Object.prototype）。
/// 仅当节点带源切片且 `val` 与解析时值 SameValue-as 时注入 own `source`
/// 属性（w/e/c 全 true）——被 reviver 前向替换的值、array/object 节点
/// 与无节点处一律空 context。
fn build_context<H: VmHost>(vm: &mut H, val: JsValue, node: Option<&JsonNode>) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    if let Some(n) = node {
        if let Some(src) = n.source.as_ref() {
            if same_value_as(vm, val, n) {
                let source_si = vm.kernel_core().perm_interner().intern("source").0;
                let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), source_si);
                obj.set_shape_id(new_shape);
                obj.ensure_hash_props().push(vm.new_string(src));
            }
        }
    }
    let obj_ptr = vm.alloc_object(obj);
    JsValue::from_js_object(obj_ptr)
}

/// SameValue-as 判定（当前值 vs 解析时值）：数字区分 +0/-0、NaN 自反；
/// 字符串按内容比；布尔/null 按值。
fn same_value_as<H: VmHost>(vm: &H, val: JsValue, node: &JsonNode) -> bool {
    match &node.kind {
        JsonKind::Null => val.is_null(),
        JsonKind::Bool(b) => val.is_bool() && val.as_bool() == *b,
        JsonKind::Number(n) => val.is_double() && same_value_number(val.as_double(), *n),
        JsonKind::String(s) => {
            if !val.is_string() {
                return false;
            }
            let parsed: Vec<u16> = s.encode_utf16().collect();
            vm.string_units(val).as_ref() == parsed.as_slice()
        }
        _ => false,
    }
}

fn same_value_number(a: f64, b: f64) -> bool {
    if a.is_nan() {
        return b.is_nan();
    }
    if a == b {
        // SameValue：+0 与 -0 不相等。
        a != 0.0 || a.is_sign_positive() == b.is_sign_positive()
    } else {
        false
    }
}

/// 子节点递归返回值施加于容器：`undefined` → [[Delete]]（非可配置保留），
/// 否则 CreateDataProperty（非可配置静默保旧值，新键建自身）。两失败路径
/// 均静默不抛（规范明注）。
fn apply_child_result<H: VmHost>(vm: &mut H, child_ptr: *mut JsObject, child_si: u32, new_val: JsValue) {
    if new_val.is_undefined() {
        let _ = crate::object::delete_own_property(vm, unsafe { &mut *child_ptr }, child_si);
    } else {
        let _ = vm.define_data_property(
            unsafe { &mut *child_ptr },
            child_si,
            new_val,
            PropAttributes::new(true, true, true),
        );
    }
}

/// 解析节点树 → JS 值树。对象键经同一字符串规范化（规范数字串映射整数键），
/// 重复键末写胜、键序字典序（与旧解析器语义一致）。
fn build_js_value<H: VmHost>(vm: &mut H, node: &JsonNode) -> JsValue {
    match &node.kind {
        JsonKind::Null => JsValue::null(),
        JsonKind::Bool(b) => JsValue::bool(*b),
        JsonKind::Number(n) => JsValue::float(*n),
        JsonKind::String(s) => vm.new_string(s),
        JsonKind::Array(items) => {
            let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
            let n = items.len();
            let array_obj = vm.alloc_object(JsObject::new_array(
                EMPTY_SHAPE_ID,
                JsValue::from_js_object(array_proto),
                n,
                vm.epoch().bump(),
            ));
            for (i, item) in items.iter().enumerate() {
                let jsv = build_js_value(vm, item);
                unsafe {
                    (*array_obj).set_prop_at(i, jsv);
                }
            }
            JsValue::from_js_object(array_obj)
        }
        JsonKind::Object(entries) => {
            // 重复键末写胜；键序字典序（与旧解析器语义一致）。
            let mut unique: Vec<(&String, &JsonNode)> = Vec::new();
            for (key, val) in entries {
                match unique.iter_mut().find(|(k, _)| *k == key.as_str()) {
                    Some(slot) => slot.1 = val,
                    None => unique.push((key, val)),
                }
            }
            unique.sort_by(|a, b| a.0.cmp(b.0));

            let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
            let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
            for (key, val) in unique {
                // 键经字符串规范化：规范数字串（"0"/"5"）映射整数键，与属性访问统一。
                let si = vm.string_key_si(key);
                let jsv = build_js_value(vm, val);
                let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
                obj.set_shape_id(new_shape);
                obj.ensure_hash_props().push(jsv);
            }
            let obj_ptr = vm.alloc_object(obj);
            JsValue::from_js_object(obj_ptr)
        }
    }
}

fn create_wrapper<H: VmHost>(vm: &mut H, value: JsValue) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let empty_si = vm.kernel_core().perm_interner().intern("").0;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), empty_si);
    obj.set_shape_id(new_shape);
    obj.ensure_hash_props().push(value);
    let obj_ptr = vm.alloc_object(obj);
    JsValue::from_js_object(obj_ptr)
}

/// space 参数文本化（Stringify 步 5）：数字与装箱 Number 经 ToNumber →
/// ToIntegerOrInfinity 钳 10；字符串与装箱 String 取前 10 字符（装箱 String
/// 走完整 ToString，步 4c 同款语义，抛出值原样上抛）；其余形态（装箱 Boolean、
/// 普通对象、Symbol 等）gap 为空串。
fn process_space<H: VmHost>(vm: &mut H, val: JsValue) -> Result<String, JsValue> {
    if val.is_int() || val.is_double() {
        let n = oxide_runtime_api::to_integer_or_infinity(val);
        if n.is_nan() || n.is_infinite() || n <= 0.0 {
            return Ok(String::new());
        }
        let clamped = (n as usize).min(10);
        return Ok(" ".repeat(clamped));
    }
    if val.is_string() {
        let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
        return Ok(s.chars().take(10).collect());
    }
    if val.is_object() {
        let ptr = val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            // [[NumberData]]：ToNumber（ToPrimitive number hint，用户方法可触发）
            // 后 ToIntegerOrInfinity。
            if obj.is_number_obj() {
                let n = match oxide_runtime_api::to_number_full(val, vm) {
                    Ok(n) => n,
                    Err(msg) => {
                        return Err(vm
                            .take_uncaught_value()
                            .unwrap_or_else(|| crate::error::create_type_error(vm, &msg)));
                    }
                };
                let n = oxide_runtime_api::to_integer_or_infinity(JsValue::float(n));
                if n.is_nan() || n.is_infinite() || n <= 0.0 {
                    return Ok(String::new());
                }
                let clamped = (n as usize).min(10);
                return Ok(" ".repeat(clamped));
            }
            // [[StringData]]：完整 ToString。
            if obj.is_string_obj() {
                let s = match oxide_runtime_api::to_string_value_full(val, vm) {
                    Ok(s) => s,
                    Err(msg) => {
                        return Err(vm
                            .take_uncaught_value()
                            .unwrap_or_else(|| crate::error::create_type_error(vm, &msg)));
                    }
                };
                let s = unsafe { (*s.as_string_ptr()).to_owned_string() };
                return Ok(s.chars().take(10).collect());
            }
        }
    }
    Ok(String::new())
}

/// 键 si 物化为单元序列：整数键 → ASCII 数字串，字符串键 → 码表键经 `decode_key`
/// 还原（含孤立 surrogate 单元）。序列化文本与 toJSON/replacer 键参数均用此真实串。
fn key_si_to_units<H: VmHost>(vm: &H, si: u32) -> Vec<u16> {
    if is_int_key(si) {
        int_key_value(si).to_string().encode_utf16().collect()
    } else {
        vm.kernel_core()
            .perm_interner()
            .lookup(si)
            .map(oxide_kernel::string_forge::decode_key)
            .unwrap_or_default()
    }
}

/// 白名单元素 → 键名提取（Stringify 步 4）：String/Number 原语直取；装箱
/// String/Number 经 ToPrimitive string hint（toString 优先、valueOf 兜底、
/// 两法皆不可调用落盒值）；其余形态（装箱 Boolean、普通对象、Symbol 等）
/// 返回 None 跳过。方法读取与调用异常传播原始抛出值。
fn whitelist_element_name<H: VmHost>(vm: &mut H, elem: JsValue) -> Result<Option<String>, JsValue> {
    if elem.is_undefined() {
        return Ok(None);
    }
    if elem.is_string() || elem.is_int() || elem.is_double() {
        return Ok(Some(oxide_runtime_api::to_string(elem)));
    }
    if !elem.is_object() {
        return Ok(None);
    }
    let ptr = elem.as_js_object_ptr();
    if ptr.is_null() {
        return Ok(None);
    }
    let obj = unsafe { &*ptr };
    let boxed = obj.boxed_value();
    if !boxed.is_string() && !boxed.is_int() && !boxed.is_double() {
        return Ok(None);
    }

    // ToPrimitive string hint：toString 优先，valueOf 兜底。
    let to_string_si = vm.kernel_core().perm_interner().intern("toString").0;
    let value_of_si = vm.kernel_core().perm_interner().intern("valueOf").0;
    for m_si in [to_string_si, value_of_si] {
        let m = vm.ordinary_get(obj, m_si, elem).map_err(|msg| {
            vm.take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
        })?;
        if !m.is_object() {
            continue;
        }
        let fptr = m.as_js_object_ptr();
        if fptr.is_null() || !unsafe { (*fptr).is_function() } {
            continue;
        }
        let r = vm.call_function_sync(m, elem, &[]).map_err(|msg| {
            vm.take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
        })?;
        return Ok(Some(oxide_runtime_api::to_string(r)));
    }

    // 装箱原型面理论不可达兜底：盒值直取。
    Ok(Some(oxide_runtime_api::to_string(boxed)))
}

fn call_to_json<H: VmHost>(vm: &mut H, obj_val: JsValue, key: &[u16]) -> Result<JsValue, JsValue> {
    // SerializeJSONProperty 步 2：对象与 BigInt 原语均查 toJSON（Get 对原语
    // 自动装箱），BigInt 的查表对象取 %BigIntPrototype%。
    if !obj_val.is_object() && !obj_val.is_bigint() {
        return Ok(obj_val);
    }
    let obj_ptr = if obj_val.is_object() {
        let p = obj_val.as_js_object_ptr();
        if p.is_null() {
            return Ok(obj_val);
        }
        p
    } else {
        vm.session().builtin_world().bigint_proto.as_ptr() as *mut JsObject
    };
    let tojson_si = vm.kernel_core().perm_interner().intern("toJSON").0;
    // Get 语义查表：访问器形 toJSON 触发 getter（receiver = 原值，BigInt 原语
    // 以自身作 this），getter 异常传播原始抛出值。
    let fn_val = match vm.ordinary_get(unsafe { &*obj_ptr }, tojson_si, obj_val) {
        Ok(v) => v,
        Err(msg) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            return Err(exc);
        }
    };
    if fn_val.is_object() {
        let fn_ptr = fn_val.as_js_object_ptr();
        if !fn_ptr.is_null() && unsafe { (*fn_ptr).is_function() } {
            let key_val = vm.new_string_units_owned(key.to_vec());
            match vm.call_function_sync(fn_val, obj_val, &[key_val]) {
                Ok(v) => Ok(v),
                Err(msg) => {
                    // 用户抛出的原始值原样传播（toJSON 抛非 Error 值不降级为 TypeError）。
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    Err(exc)
                }
            }
        } else {
            Ok(obj_val)
        }
    } else {
        Ok(obj_val)
    }
}

/// 序列化族单自身属性值读（SerializeJSONProperty 步 2 的 Get）：数据属性直读
/// 存储槽；访问器属性触发 getter（this = 容器对象），getter 异常传播原始
/// 抛出值。
fn read_json_property_value<H: VmHost>(
    vm: &mut H, obj: &JsObject, obj_val: JsValue, si: u32, pos: u32,
) -> Result<JsValue, JsValue> {
    if obj.is_accessor_meta(pos) {
        match vm.ordinary_get(obj, si, obj_val) {
            Ok(value) => Ok(value),
            Err(msg) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                Err(exc)
            }
        }
    } else {
        Ok(obj.get_prop_at(pos))
    }
}

/// `JSON.stringify(value, replacer, space)`：将 JS 值序列化为 JSON 文本。
/// 支持 replacer 函数/属性白名单、toJSON 钩子、缩进与循环引用检测。
pub fn json_stringify<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let value = vm.reg(args[1]);
    if value.is_undefined() {
        return NativeResult::Ok(JsValue::undefined());
    }

    let mut replacer_fn: Option<JsValue> = None;
    let mut replacer_whitelist: Option<Vec<String>> = None;
    if args.len() > 2 {
        let replacer_val = vm.reg(args[2]);
        if replacer_val.is_object() {
            let rptr = replacer_val.as_js_object_ptr();
            if !rptr.is_null() {
                let robj = unsafe { &*rptr };
                if robj.is_function() {
                    replacer_fn = Some(replacer_val);
                } else if robj.is_array() {
                    let len = robj.prop_count() as usize;
                    let mut whitelist: Vec<String> = Vec::new();
                    for i in 0..len {
                        let elem = robj.get_prop_at(i);
                        match whitelist_element_name(vm, elem) {
                            Ok(Some(name)) => {
                                // 有序去重：重复项不入列表。
                                if !whitelist.contains(&name) {
                                    whitelist.push(name);
                                }
                            }
                            Ok(None) => {}
                            Err(exc) => return NativeResult::Err(exc),
                        }
                    }
                    replacer_whitelist = Some(whitelist);
                }
            }
        }
    }

    let space = if args.len() > 3 {
        match process_space(vm, vm.reg(args[3])) {
            Ok(s) => s,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        String::new()
    };

    let holder = create_wrapper(vm, value);

    let value = match call_to_json(vm, value, &[]) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };

    let value = if let Some(replacer) = replacer_fn {
        let key_val = vm.new_string("");
        match vm.call_function_sync(replacer, holder, &[key_val, value]) {
            Ok(v) => v,
            Err(msg) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                return NativeResult::Err(exc);
            }
        }
    } else {
        value
    };

    // 顶层省略值：undefined、函数、Symbol 一律返回 undefined（非空串）。
    let top_is_fn = value.is_object() && {
        let p = value.as_js_object_ptr();
        !p.is_null() && unsafe { (*p).is_function() }
    };
    if value.is_undefined() || value.is_symbol() || top_is_fn {
        return NativeResult::Ok(JsValue::undefined());
    }

    let mut visited = HashSet::new();
    let mut output = String::new();
    let indent_level: usize = 0;
    match jsvalue_to_json(
        vm,
        value,
        &mut visited,
        &mut output,
        replacer_fn,
        replacer_whitelist.as_ref(),
        &space,
        indent_level,
    ) {
        Ok(()) => NativeResult::Ok(vm.new_string_owned(output)),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// 值位序列化（SerializeValue 及对象/数组展开）。toJSON 钩子不在本入口应用——
/// 唯一应用点在调用方（顶层 `json_stringify` 与 `stringify_object`/`stringify_array`
/// 的属性循环内，Get 之后、replacer 之前），保证每值位恰一次且与 replacer 顺序
/// 符合规范。`Err` 携带的原始异常值（含环检 TypeError）原样上抛。
#[allow(clippy::too_many_arguments)]
fn jsvalue_to_json<H: VmHost>(
    vm: &mut H, val: JsValue, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    if val.is_null() {
        out.push_str("null");
    } else if val.is_undefined() {
    } else if val.is_symbol() {
        out.push_str("null");
    } else if val.is_bool() {
        out.push_str(if val.as_bool() { "true" } else { "false" });
    } else if val.is_int() {
        let _ = write!(out, "{}", val.as_int());
    } else if val.is_double() {
        let n = val.as_double();
        if !n.is_finite() {
            out.push_str("null");
        } else {
            oxide_runtime_api::write_number_into(n, out);
        }
    } else if val.is_bigint() {
        // SerializeJSONProperty 步 10：BigInt 无 JSON 文本形态，抛 TypeError。
        return Err(crate::error::create_type_error(vm, "Do not know how to serialize a BigInt"));
    } else if val.is_string() {
        // SAFETY: val 已确认是字符串值；单元视图按码单元流序列化（见 stringify_string_units）。
        let units = unsafe { (*val.as_string_ptr()).units() };
        stringify_string_units(&units, out);
    } else if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if obj_ptr.is_null() {
            out.push_str("null");
            return Ok(());
        }

        // SerializeJSONProperty 步 1a：raw JSON 对象直接输出原始文本（先于
        // toJSON 查找；该对象仅一个字符串属性，不可能参与环，免环检）。
        {
            let obj = unsafe { &*obj_ptr };
            if obj.is_raw_json_obj() {
                let raw_val = obj.get_prop_at(0);
                // SAFETY: rawJSON 属性构造期即写入字符串值。
                let raw_text = unsafe { (*raw_val.as_string_ptr()).to_owned_string() };
                out.push_str(&raw_text);
                return Ok(());
            }
        }

        if !visited.insert(obj_ptr as *const JsObject) {
            return Err(crate::error::create_type_error(vm, "Converting circular structure to JSON"));
        }

        let obj = unsafe { &*obj_ptr };
        // 步 4a：装箱 Number → ToNumber（用户方法可触发），非有限值 → null。
        if obj.is_number_obj() {
            let n = match oxide_runtime_api::to_number_full(val, vm) {
                Ok(n) => n,
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    visited.remove(&(obj_ptr as *const JsObject));
                    return Err(exc);
                }
            };
            if n.is_finite() {
                oxide_runtime_api::write_number_into(n, out);
            } else {
                out.push_str("null");
            }
            visited.remove(&(obj_ptr as *const JsObject));
            return Ok(());
        }
        // 步 4b：装箱 Boolean → [[BooleanData]]。
        if obj.is_boolean_obj() {
            let b = obj.boxed_value();
            out.push_str(if b.is_bool() && b.as_bool() { "true" } else { "false" });
            visited.remove(&(obj_ptr as *const JsObject));
            return Ok(());
        }
        // 步 4d：装箱 BigInt 无条件解包 [[BigIntData]]，后续步 10 抛 TypeError。
        if obj.boxed_value().is_bigint() {
            return Err(crate::error::create_type_error(vm, "Do not know how to serialize a BigInt"));
        }
        // 步 4c：装箱 String 走完整 ToString（[[StringData]] 臂）：尊重
        // toString/Symbol.toPrimitive 覆盖、不直读载荷，抛出值原样传播。
        if obj.is_string_obj() {
            let s = match oxide_runtime_api::to_string_value_full(val, vm) {
                Ok(s) => s,
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    visited.remove(&(obj_ptr as *const JsObject));
                    return Err(exc);
                }
            };
            let units = vm.string_units(s);
            stringify_string_units(&units, out);
        } else if obj.is_typed_array_obj() {
            stringify_typed_array(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        } else if obj.is_array() {
            stringify_array(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        } else {
            stringify_object(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        }

        visited.remove(&(obj_ptr as *const JsObject));
    }
    Ok(())
}

/// JSON 字符串序列化（按码单元流）：代理对原样输出 astral 字符，非配对孤立
/// surrogate 输出 \uXXXX（well-formed JSON 要求，round-trip 经 JSON.parse 复原同值），
/// C0/C1 控制码输出 \uXXXX，其余按 JSON 转义规则。
fn stringify_string_units(units: &[u16], out: &mut String) {
    out.push('"');
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u) && i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
            let cp = 0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(units[i + 1]) - 0xDC00);
            if let Some(c) = char::from_u32(cp) {
                out.push(c);
            }
            i += 2;
            continue;
        }
        match u {
            0x22 => out.push_str("\\\""),
            0x5C => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0C => out.push_str("\\f"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x00..=0x1F | 0x7F..=0x9F | 0xD800..=0xDFFF => {
                let _ = write!(out, "\\u{:04x}", u);
            }
            _ => {
                if let Some(c) = char::from_u32(u32::from(u)) {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    out.push('"');
}

#[allow(clippy::too_many_arguments)]
fn stringify_object<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    out.push('{');

    // 键集：白名单给定时 K = P（列表序，不限自身可枚举，值读走 Get）；
    // 无白名单走自身可枚举序（EnumerableOwnPropertyNames）。
    let entries: Vec<(Vec<u16>, u32, Option<u32>)> = if let Some(whitelist) = replacer_whitelist {
        whitelist
            .iter()
            .map(|name| {
                let si = vm.string_key_si(name);
                (key_si_to_units(vm, si), si, None)
            })
            .collect()
    } else {
        let keys = walk_own_keys(vm, obj);
        keys.into_iter()
            .filter(|(_si, pos)| {
                obj.prop_meta_at(*pos)
                    .map(|m| m.attributes.enumerable())
                    .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
            })
            .map(|(si, pos)| (key_si_to_units(vm, si), si, Some(pos)))
            .collect()
    };

    let obj_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let mut first = true;
    for (units, si, read_pos) in entries {
        // 规范序 Get → toJSON → replacer（SerializeJSONProperty 步 1、2a、2b）。
        // K = P 时值读经 Get（原型链 + 访问器），无白名单自身槽直读。
        let val = match read_pos {
            Some(pos) => read_json_property_value(vm, obj, obj_val, si, pos)?,
            None => vm.ordinary_get(obj, si, obj_val).map_err(|msg| {
                vm.take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
            })?,
        };
        let val = call_to_json(vm, val, &units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string_units_owned(units.clone());
            match vm.call_function_sync(replacer, obj_val, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        continue;
                    }
                    v
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return Err(exc);
                }
            }
        } else {
            val
        };

        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            !ptr.is_null() && unsafe { (*ptr).is_function() }
        };
        if val.is_undefined() || val.is_symbol() || is_function {
            continue;
        }

        if !first {
            out.push(',');
        }
        first = false;

        if has_space {
            out.push('\n');
            for _ in 0..indent_level + 1 {
                out.push_str(space);
            }
            stringify_string_units(&units, out);
            out.push(':');
            out.push(' ');
        } else {
            stringify_string_units(&units, out);
            out.push(':');
        }

        jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1)?;
    }

    if has_space && !first {
        out.push('\n');
        for _ in 0..indent_level {
            out.push_str(space);
        }
    }
    out.push('}');
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stringify_array<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    out.push('[');

    let obj_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let len = obj.prop_count() as usize;
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }

        if has_space {
            out.push('\n');
            for _ in 0..indent_level + 1 {
                out.push_str(space);
            }
        }

        let index_str = i.to_string();
        let index_units: Vec<u16> = index_str.encode_utf16().collect();

        // 规范序 Get → toJSON → replacer（数组元素位同款，getter 的 this = 数组自身）。
        let val = read_json_property_value(vm, obj, obj_val, make_int_key(i as u32), i as u32)?;
        let val = call_to_json(vm, val, &index_units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string(&index_str);
            match vm.call_function_sync(replacer, obj_val, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        out.push_str("null");
                        continue;
                    }
                    v
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return Err(exc);
                }
            }
        } else {
            val
        };

        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            !ptr.is_null() && unsafe { (*ptr).is_function() }
        };
        if is_function || val.is_undefined() {
            out.push_str("null");
        } else {
            jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1)?;
        }
    }

    if has_space && len > 0 {
        out.push('\n');
        for _ in 0..indent_level {
            out.push_str(space);
        }
    }
    out.push(']');
    Ok(())
}

/// TypedArray 序列化：整数索引是可枚举自身属性（元素存于原生载荷，不在形状链），
/// 逐元素按对象臂规范序 Get → toJSON → replacer 处理，输出形态与普通对象一致。
#[allow(clippy::too_many_arguments)]
fn stringify_typed_array<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = crate::typed_array::get_typed_array_data(vm, this_val)?;
    // live 口径：buffer 收缩/detach 后按当前 live 长度枚举（detached 空对象形态）。
    let len = crate::typed_array::ta_live_length(view);
    out.push('{');

    // 键集：白名单给定时 K = P（列表序，整数索引名读元素、越界名跳过、
    // 非索引名经 Get 读命名键如 length）；无白名单走升序元素序。
    let entries: Vec<(Vec<u16>, JsValue)> = if let Some(whitelist) = replacer_whitelist {
        let mut list: Vec<(Vec<u16>, JsValue)> = Vec::new();
        for name in whitelist {
            let si = vm.string_key_si(name);
            let val = if is_int_key(si) {
                let idx = int_key_value(si);
                if idx as usize >= len {
                    continue;
                }
                crate::typed_array::typed_array_element_get(vm, obj, idx)
            } else {
                vm.ordinary_get(obj, si, this_val)
            }
            .map_err(|msg| crate::error::create_type_error(vm, &msg))?;
            list.push((key_si_to_units(vm, si), val));
        }
        list
    } else {
        let mut list: Vec<(Vec<u16>, JsValue)> = Vec::with_capacity(len);
        for i in 0..len {
            let val = crate::typed_array::typed_array_element_get(vm, obj, i as u32)
                .map_err(|msg| crate::error::create_type_error(vm, &msg))?;
            list.push((i.to_string().encode_utf16().collect(), val));
        }
        list
    };

    let mut first = true;
    for (index_units, val) in entries {
        let val = call_to_json(vm, val, &index_units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string_units_owned(index_units.clone());
            match vm.call_function_sync(replacer, this_val, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        continue;
                    }
                    v
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return Err(exc);
                }
            }
        } else {
            val
        };

        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            !ptr.is_null() && unsafe { (*ptr).is_function() }
        };
        if val.is_undefined() || val.is_symbol() || is_function {
            continue;
        }

        if !first {
            out.push(',');
        }
        first = false;

        if has_space {
            out.push('\n');
            for _ in 0..indent_level + 1 {
                out.push_str(space);
            }
            stringify_string_units(&index_units, out);
            out.push(':');
            out.push(' ');
        } else {
            stringify_string_units(&index_units, out);
            out.push(':');
        }

        jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1)?;
    }

    if has_space && !first {
        out.push('\n');
        for _ in 0..indent_level {
            out.push_str(space);
        }
    }
    out.push('}');
    Ok(())
}
