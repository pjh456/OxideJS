use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

#[test]
fn regexp_survives_gc_and_still_matches() {
    // 回归：正则存入全局对象触发晋升/GC 搬移后，native_fn 槽的已编译 Box
    // 必须深拷贝到新对象（而非共享指针），否则 epoch 释放与 teardown 双重释放。
    // 非 global 正则：test/exec 不追踪 lastIndex，跨 reset 探针不受写回干扰。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var r = /ab+c/; globalThis.r = r; r.test('xxabbcx')").unwrap();
    assert!(result.as_bool());

    // 触发完整收集：存活正则克隆进新 arena，旧 Box 由 sweep 释放。
    vm.reset();

    let result = eval(&mut vm, "globalThis.r.test('xxabbcx')").unwrap();
    assert!(result.as_bool(), "GC 后正则应仍可匹配");
    let result = eval(&mut vm, "globalThis.r.exec('xabbbcx')[0]").unwrap();
    assert_eq!(to_str(&vm, result), "abbbc");
}

// --- RegExp constructor ---

#[test]
fn regexp_constructor_creates_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof new RegExp('abc')").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn regexp_constructor_with_flags() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'gi').ignoreCase").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_constructor_invalid_pattern_syntax_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('[')");
    assert!(result.is_err() || (result.is_ok() && result.unwrap().is_string()));
}

#[test]
fn regexp_source_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('hello', 'g').source").unwrap();
    assert_eq!(to_str(&vm, result), "hello");
}

#[test]
fn regexp_flags_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'gi').flags").unwrap();
    assert_eq!(to_str(&vm, result), "gi");
}

#[test]
fn regexp_global_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'g').global").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_multiline_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'm').multiline").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_last_index_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'g').lastIndex").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn regexp_dotall_sticky_unicode_default_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc').dotAll").unwrap();
    assert!(!result.as_bool());
}

// --- RegExp.prototype.test ---

#[test]
fn regexp_test_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc').test('abc')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_test_no_match() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc').test('xyz')").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn regexp_test_case_insensitive() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new RegExp('abc', 'i').test('ABC')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_test_global_flag_iteration() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var re = /a/g; re.test('a')").unwrap();
    assert!(result.as_bool());
}

// --- RegExp.prototype.exec ---

#[test]
fn regexp_exec_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/hello/.exec('hello world') !== null").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_exec_no_match_returns_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/xyz/.exec('hello')").unwrap();
    assert!(result.is_null());
}

#[test]
fn regexp_exec_returns_index() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/ell/.exec('hello')").unwrap();
    assert!(result.is_object());
}

#[test]
fn regexp_exec_returns_input() {
    let mut vm = Vm::new();
    let m = eval(&mut vm, "/ell/.exec('hello')").unwrap();
    assert!(m.is_object());
}

#[test]
fn regexp_exec_global_last_index_advances() {
    let mut vm = Vm::new();
    let re = eval(&mut vm, "var re = /a/g; re.exec('aba'); re.lastIndex").unwrap();
    assert_eq!(re.as_int(), 1);
}

#[test]
fn regexp_exec_global_resets_on_exhaustion() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var re = /a/g; re.exec('a'); re.exec('a') === null").unwrap();
    assert!(result.as_bool());
}

// --- exec lastIndex 规范语义（sticky 锚定 / 越界短路 / 失败重置 / Set 写回）---

#[test]
fn regexp_exec_sticky_anchored_hit_advances() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var re = /b/y; re.lastIndex = 1; var m = re.exec('ab'); m[0] + ':' + m.index + ':' + re.lastIndex",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "b:1:2");
}

#[test]
fn regexp_exec_sticky_miss_not_anchored_resets() {
    // sticky 匹配起点不在 lastIndex 时判无匹配：置 0 回 null（引擎对 y 不原生锚定，靠后置过滤）。
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "var re = /b/y; re.lastIndex = 0; re.exec('ab') === null && re.lastIndex === 0").unwrap();
    assert!(result.as_bool());
    let result =
        eval(&mut vm, "var re = /c/y; re.lastIndex = 1; re.exec('ab') === null && re.lastIndex === 0").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_exec_out_of_range_resets_last_index() {
    // lastIndex 超出串长：先 Set 0 再回 null（global 与 sticky 同口径）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var re = /./g; re.lastIndex = 999; re.exec('abc') === null && re.lastIndex === 0",
    )
    .unwrap();
    assert!(result.as_bool());
    let result = eval(
        &mut vm,
        "var re = /./y; re.lastIndex = 999; re.exec('abc') === null && re.lastIndex === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_exec_nonwritable_last_index_throws() {
    // 失败 Set 0 命中不可写 lastIndex：TypeError（Set 语义 strict）。
    let source = "(() => { var re = /c/y; Object.defineProperty(re, 'lastIndex', { writable: false }); try { re.exec('ab'); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()";
    let mut vm = Vm::new();
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn regexp_exec_this_relaxed_to_object_gate() {
    // 门禁只要求对象：普通对象 this 缺编译正则槽，同样 TypeError；null/undefined 非对象，TypeError。
    for this_expr in ["{}", "null", "undefined", "42"] {
        let source = "(() => { try { RegExp.prototype.exec.call(__THIS__, 'abc'); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()"
            .replace("__THIS__", this_expr);
        let mut vm = Vm::new();
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "TypeError", "this = {this_expr}");
    }
}

#[test]
fn regexp_exec_last_index_read_via_get_only() {
    // lastIndex 读走 Get + ToLength（对象经 valueOf）；非 global/sticky 成功与失败均不写回。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var re = /a/; re.lastIndex = { valueOf: function () { return 1; } }; var m = re.exec('xa'); m[0] + ':' + m.index + ':' + (re.lastIndex instanceof Object)",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "a:1:true");
    let result = eval(
        &mut vm,
        "var re = /z/; re.lastIndex = { valueOf: function () { return 0; } }; re.exec('xa') === null && re.lastIndex instanceof Object",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_exec_negative_last_index_clamped_to_zero() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var re = /./g; re.lastIndex = -1; var m = re.exec('a'); m.index + ':' + re.lastIndex",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "0:1");
}

#[test]
fn regexp_exec_last_index_set_once_on_success() {
    // 成功路径 lastIndex 恰好 Set 一次（写回在核内单点）：计数 setter 只被观察一次。
    let source = "(() => { var re = /a/g; var n = 0; Object.defineProperty(re, 'lastIndex', { configurable: true, set() { n += 1; }, get() { return 0; } }); re.exec('a'); return String(n); })()";
    let mut vm = Vm::new();
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "1");
}

// --- test() lastIndex 规范语义（sticky 锚定 / 越界短路 / 不追踪不写回）---

#[test]
fn regexp_test_sticky_anchor_failure_resets() {
    // sticky 命中起点不在 lastIndex：判无匹配，lastIndex 置 0（后置锚定过滤）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var re = /b/y; re.test('ab') === false && re.lastIndex === 0").unwrap();
    assert!(result.as_bool());
    let result =
        eval(&mut vm, "var re = /c/y; re.lastIndex = 1; re.test('abc') === false && re.lastIndex === 0").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_test_sticky_hit_advances_last_index() {
    // 命中时 lastIndex 推进到匹配末尾；初始 lastIndex 受尊重（匹配须恰从其起点）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var re = /abc/y; re.test('abc') === true && re.lastIndex === 3").unwrap();
    assert!(result.as_bool());
    let result =
        eval(&mut vm, "var re = /./y; re.lastIndex = 1; re.test('a') === false && re.lastIndex === 0").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_test_sticky_out_of_range_resets_last_index() {
    // lastIndex 超出串长：先 Set 0 再回 false（sticky 与 global 同口径）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var re = /./y; re.lastIndex = 999; re.test('abc') === false && re.lastIndex === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_test_sticky_nonwritable_last_index_throws() {
    // 失败 Set 0 命中不可写 lastIndex：TypeError（Set 语义 strict）。
    let source = "(() => { var re = /c/y; Object.defineProperty(re, 'lastIndex', { writable: false }); try { re.test('abc'); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()";
    let mut vm = Vm::new();
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn regexp_test_non_global_no_write_back() {
    // 非 global/sticky：无 lastIndex 写回，不可写属性静默通过（不触发 Set）。
    let source = "(() => { var re = /a/; Object.defineProperty(re, 'lastIndex', { writable: false, value: 7 }); return re.test('xa') === true && re.lastIndex === 7 && re.test('z') === false && re.lastIndex === 7; })()";
    let mut vm = Vm::new();
    let result = eval(&mut vm, source).unwrap();
    assert!(result.as_bool());
}

// --- RegExp literal compilation ---

#[test]
fn regexp_literal_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/hello/").unwrap();
    assert!(result.is_object());
}

#[test]
fn regexp_literal_with_flags() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/abc/i.test('ABC')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_literal_global_exec() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var re = /a/g; re.exec('aba'); re.lastIndex").unwrap();
    assert_eq!(result.as_int(), 1);
}

// --- String integration ---

#[test]
fn string_match_with_regexp() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello world'.match(/ell/)").unwrap();
    assert!(result.is_object());
}

#[test]
fn string_replace_with_regexp() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.replace(/ell/, 'ipp')").unwrap();
    assert_eq!(to_str(&vm, result), "hippo");
}

#[test]
fn string_replace_with_regexp_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aba'.replace(/a/g, 'c')").unwrap();
    assert_eq!(to_str(&vm, result), "cbc");
}

#[test]
fn string_search_with_regexp() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.search(/ell/)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn string_search_not_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.search(/xyz/)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_split_with_regexp() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a,b,c'.split(/,/)").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "b");
}

// --- toString ---

#[test]
fn regexp_to_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/abc/gi.toString()").unwrap();
    assert_eq!(to_str(&vm, result), "/abc/gi");
}

// --- RegExp.escape 静态方法 ---

#[test]
fn regexp_escape_table_representatives() {
    let mut vm = Vm::new();
    // 首字符 ASCII 字母先走 \xNN 规则，其余语法字符反斜杠加字符本身。
    let result = eval(&mut vm, "RegExp.escape('a.b')").unwrap();
    assert_eq!(to_str(&vm, result), "\\x61\\.b");
    let result = eval(&mut vm, "RegExp.escape('.a1')").unwrap();
    assert_eq!(to_str(&vm, result), "\\.a1");
    // 控制字符：单字母转义形态。
    let result = eval(&mut vm, "RegExp.escape('\\t\\n\\v\\f\\r')").unwrap();
    assert_eq!(to_str(&vm, result), "\\t\\n\\v\\f\\r");
    // 空白：≤0xFF 走 \xNN，扩展 USP 段与 BOM 走 \uNNNN（十六进制小写）。
    let result = eval(&mut vm, "RegExp.escape(' ')").unwrap();
    assert_eq!(to_str(&vm, result), "\\x20");
    let result = eval(&mut vm, "RegExp.escape('\\uFEFF')").unwrap();
    assert_eq!(to_str(&vm, result), "\\ufeff");
    let result = eval(&mut vm, "RegExp.escape('\\u202F')").unwrap();
    assert_eq!(to_str(&vm, result), "\\u202f");
    // 其它标点。
    let result = eval(&mut vm, "RegExp.escape(',')").unwrap();
    assert_eq!(to_str(&vm, result), "\\x2c");
    // 首字符数字/ASCII 字母：\xNN；非首位不转义。
    let result = eval(&mut vm, "RegExp.escape('1111')").unwrap();
    assert_eq!(to_str(&vm, result), "\\x31111");
    let result = eval(&mut vm, "RegExp.escape('aaa')").unwrap();
    assert_eq!(to_str(&vm, result), "\\x61aa");
    // 下划线不转义。
    let result = eval(&mut vm, "RegExp.escape('_hello')").unwrap();
    assert_eq!(to_str(&vm, result), "_hello");
}

#[test]
fn regexp_escape_surrogates_and_non_bmp() {
    let mut vm = Vm::new();
    // 孤立 surrogate：\uXXXX（4 位小写十六进制）。
    let result = eval(&mut vm, "RegExp.escape('\\uD800')").unwrap();
    assert_eq!(to_str(&vm, result), "\\ud800");
    // 非 BMP 码点原样（代理对不按孤立 surrogate 转义）。
    let result = eval(&mut vm, "RegExp.escape('\\u{1F600}')").unwrap();
    assert_eq!(to_str(&vm, result), "\u{1F600}");
}

#[test]
fn regexp_escape_non_string_throws_type_error() {
    let mut vm = Vm::new();
    let source = "(() => { try { RegExp.escape(123); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()";
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn regexp_escape_length_and_name() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "RegExp.escape.length + ':' + RegExp.escape.name").unwrap();
    assert_eq!(to_str(&vm, result), "1:escape");
}

// --- Unicode property escapes：Script/Script_Extensions 的 Unknown（Zzzz）取值 ---
// vendor 的 Unicode 表须含 Unknown（未分配码点集），否则合法模式编译期 SyntaxError。

#[test]
fn regexp_unicode_property_script_unknown_matches_unassigned() {
    let mut vm = Vm::new();
    // U+038B 未分配：Script 与 Script_Extensions 均为 Unknown
    let result = eval(&mut vm, "/\\p{Script=Unknown}/u.test(String.fromCodePoint(0x038B))").unwrap();
    assert!(result.as_bool(), "未分配码点应匹配 Script=Unknown");
    let result = eval(&mut vm, "/\\p{Script_Extensions=Unknown}/u.test(String.fromCodePoint(0x038B))").unwrap();
    assert!(result.as_bool(), "未分配码点应匹配 Script_Extensions=Unknown");
}

#[test]
fn regexp_unicode_property_script_unknown_rejects_assigned() {
    let mut vm = Vm::new();
    // 已分配拉丁字母不属于 Unknown
    let result = eval(&mut vm, "/\\p{Script=Unknown}/u.test('d')").unwrap();
    assert!(!result.as_bool(), "已分配码点不应匹配 Script=Unknown");
    let result = eval(&mut vm, "/\\P{Script=Unknown}/u.test('d')").unwrap();
    assert!(result.as_bool(), "取反后已分配码点应匹配");
}

#[test]
fn regexp_unicode_property_script_unknown_aliases() {
    let mut vm = Vm::new();
    // 单字母缩写与符号名须同义解析
    for pattern in [r"\p{sc=Unknown}", r"\p{scx=Unknown}", r"\p{Script=Zzzz}", r"\p{scx=Zzzz}"] {
        let source = format!("/{pattern}/u.test(String.fromCodePoint(0x038B))");
        let result = eval(&mut vm, &source).unwrap();
        assert!(result.as_bool(), "别名 {pattern} 应匹配未分配码点");
    }
}

#[test]
fn regexp_unicode_property_script_existing_values_unchanged() {
    // 补齐 Unknown 不得扰动既有取值：Adlam 等已分配 Script 行为保持
    let mut vm = Vm::new();
    let result = eval(&mut vm, "/\\p{Script=Adlam}/u.test(String.fromCodePoint(0x1E900))").unwrap();
    assert!(result.as_bool(), "既有 Script 取值行为应保持不变");
    let result = eval(&mut vm, "/\\p{Script=Adlam}/u.test('d')").unwrap();
    assert!(!result.as_bool(), "既有 Script 取值行为应保持不变");
}

// --- UTF-16 单元向匹配入口：gate 运行时化 + 惰性 units IR 回归钉 ---
// 引擎构建已启用 regress 的 utf16 feature（纯加性 API）。字节字面量 pass 改为
// 逐编译运行时 gate 后，str 路径编译与执行不变；单元向 IR 在首次
// find_from_utf16 时惰性编译（关字节 pass）并缓存。以下钉直接走 regress API，
// 不依赖 JS 侧单元视图接线（后续批次才落地）。

fn units_of(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn regexp_utf16_entry_surrogate_property() {
    // 孤立 surrogate 单元命中 \p{General_Category=Surrogate}；普通字符不命中；
    // 代理对（单码点）不命中。
    let re = regress::Regex::with_flags(r"\p{General_Category=Surrogate}", "u").unwrap();
    let m = re.find_from_utf16(&[0xD800], 0).next();
    assert!(m.is_some(), "孤立 surrogate 单元应命中 GC=Surrogate");
    assert_eq!(m.unwrap().range(), 0..1);
    assert!(re.find_from_utf16(&[0x41], 0).next().is_none(), "A 不应命中");
    assert!(
        re.find_from_utf16(&[0xD834, 0xDE00], 0).next().is_none(),
        "代理对是单码点，不应按两个孤立 surrogate 命中"
    );
}

#[test]
fn regexp_utf16_entry_surrogate_pair_equals_str_path() {
    // U+1D11E = 代理对 [0xD834, 0xDE00]：单元路径按单码点匹配，与 str 路径等价。
    let ch = char::from_u32(0x1D11E).unwrap();
    let re = regress::Regex::with_flags(&ch.to_string(), "").unwrap();
    let text: String = [ch, 'a', 'b', 'c'].iter().copied().collect();
    let m_str = re.find(&text).expect("str 路径应命中");
    assert_eq!(m_str.range(), 0..4, "str 路径命中 1 个码点（4 字节）");

    let units = units_of(&text);
    let m_u16 = re.find_from_utf16(&units, 0).next().expect("单元路径应命中");
    assert_eq!(m_u16.range(), 0..2, "单元路径命中 1 个码点（2 个单元）");
}

#[test]
fn regexp_utf16_entry_well_formed_equals_str_path() {
    // well-formed 语料：单元路径与 str 路径匹配结果逐一相等（含非 ASCII）。
    for (pattern, text) in [(r"\d+", "abc123x456"), (r"a[bc]d", "xabcdyabd"), (r"\w+\s+\w+", "héllo wörld")] {
        let re = regress::Regex::new(pattern).unwrap();
        let str_m = re.find(text).map(|m| m.as_str(text).to_string());
        let units = units_of(text);
        let u16_m = re.find_from_utf16(&units, 0).next().map(|m| {
            units[m.range()]
                .iter()
                .flat_map(|&u| char::from_u32(u as u32).map(|c| c.to_string()))
                .collect::<String>()
        });
        assert_eq!(str_m, u16_m, "pattern {pattern}");
    }
}

// --- RegExp flag 只读访问器（M11）---

#[test]
fn regexp_flag_accessors_read_values() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var r = new RegExp('a', 'gimsyud'); [r.global, r.ignoreCase, r.multiline, r.dotAll, r.sticky, r.unicode, r.hasIndices, r.unicodeSets].join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "true,true,true,true,true,true,true,false");
    let result = eval(&mut vm, "new RegExp('a', 'v').unicodeSets").unwrap();
    assert!(result.as_bool());
    let result =
        eval(&mut vm, "new RegExp('a', 'u').unicodeSets === false && new RegExp('a').unicode === false").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_flag_accessor_descriptor_and_identity() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(RegExp.prototype, 'global'); d.set === undefined && typeof d.get === 'function' && d.enumerable === false && d.configurable === true && d.get.length === 0 && d.get.name === 'get global'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_instance_has_no_own_flag_props() {
    let mut vm = Vm::new();
    // source/flags 是原型访问器，非实例自身属性；仅 lastIndex 为自身数据属性。
    let result = eval(&mut vm, "Object.getOwnPropertyNames(new RegExp('a', 'g')).sort().join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "lastIndex");
}

#[test]
fn regexp_flag_write_is_noop_sloppy() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var r = new RegExp('a'); r.global = true; r.ignoreCase = false; r.global === false",
    )
    .unwrap();
    assert!(result.as_bool(), "无 setter 访问器在 sloppy 写应静默 no-op");
}

#[test]
fn regexp_uv_flags_mutual_exclusive() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { try { new RegExp('.', 'uv'); return 'no-throw'; } catch (e) { return e instanceof SyntaxError ? 'SyntaxError' : 'other'; } })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "SyntaxError");
    // u 与 v 单独使用不受影响。
    let result = eval(&mut vm, "new RegExp('.', 'u').unicode + ',' + new RegExp('.', 'v').unicodeSets").unwrap();
    assert_eq!(to_str(&vm, result), "true,true");
}

#[test]
fn regexp_flag_getter_proto_this_returns_undefined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'dotAll').get.call(RegExp.prototype) === undefined",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_flag_getter_non_regexp_this_throws_type_error() {
    let mut vm = Vm::new();
    let source = "(() => { var get = Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').get; try { get.call({}); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()";
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
    let source = "(() => { var get = Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').get; try { get.call(null); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()";
    let result = eval(&mut vm, source).unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn symbol_match_live_global_shadow_and_exec_result() {
    let mut vm = Vm::new();
    // 影子 global 数据属性压过原型访问器（非 g 正则走 global 循环）。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; var n = 0; r.exec = function() { n += 1; return n === 1 ? ['b'] : null; }; Object.defineProperty(r, 'global', { value: true, writable: true, configurable: true }); return r[Symbol.match]('abc').join(','); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "b");
    // 非 global 直接返回 exec 结果原值（对象恒等）。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; var marker = { tag: 1 }; r.exec = function() { return marker; }; return r[Symbol.match]('abc') === marker; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn symbol_match_live_flag_reads_propagate() {
    let mut vm = Vm::new();
    // flags getter 抛错原样传播（unicode 读先于非 global 返回，读序不短路）。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; Object.defineProperty(r, 'flags', { get() { throw new RangeError('flags-boom'); } }); try { r[Symbol.match]('abc'); return 'no-throw'; } catch (e) { return e.message; } })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "flags-boom");
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; Object.defineProperty(r, 'unicode', { get() { throw new RangeError('unicode-boom'); } }); try { r[Symbol.match]('abc'); return 'no-throw'; } catch (e) { return e.message; } })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "unicode-boom");
}

#[test]
fn symbol_entries_accept_plain_object_this() {
    let mut vm = Vm::new();
    // this 放宽：普通对象（带 exec）可作 match/replace/search 的 this。
    let result = eval(
        &mut vm,
        "(() => { var o = { exec: function() { return ['x', { index: 0 }]; } }; return RegExp.prototype[Symbol.replace].call(o, 'abc', 'Y'); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "Ybc");
    let result = eval(
        &mut vm,
        "(() => { var o = { exec: function() { return { index: 2 }; } }; return RegExp.prototype[Symbol.search].call(o, 'abcd'); })()",
    )
    .unwrap();
    // ToNumber 结果为浮点形态（2.0）。
    assert_eq!(result.as_double(), 2.0);
}

#[test]
fn symbol_replace_functional_groups_tail_arg() {
    let mut vm = Vm::new();
    // groups 非 undefined 时作末参（5 参）；undefined 时原串为末参（4 参）。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; r.exec = function() { return { length: 1, 0: 'b', index: 1, groups: { g: 1 } }; }; return r[Symbol.replace]('abc', function() { return arguments.length; }); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "a4c");
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; r.exec = function() { return { length: 1, 0: 'b', index: 1 }; }; return r[Symbol.replace]('abc', function() { return arguments.length; }); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "a3c");
}

#[test]
fn symbol_replace_out_of_order_position_ignored() {
    let mut vm = Vm::new();
    // position 回退的替换被忽略（乱序结果）。
    let result = eval(
        &mut vm,
        "(() => { var r = /./g; var n = 0; r.exec = function() { n += 1; if (n === 1) return { index: 1, length: 1, 0: 'x' }; if (n === 2) return { index: 1, length: 1, 0: 'y' }; return null; }; return r[Symbol.replace]('abcde', 'Z'); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "aZcde");
}

#[test]
fn symbol_search_lastindex_init_and_restore() {
    let mut vm = Vm::new();
    // previous 与 +0 非 SameValue 时先 Set 0；exec 后按原值恢复。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/g; var seen = null; r.lastIndex = 3; r.exec = function() { seen = r.lastIndex; return { index: 1 }; }; r[Symbol.search]('abcd'); return seen + '/' + r.lastIndex; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "0/3");
    // 严格 SameValue 口径：-0 与 +0 不同值，初始化 Set 0 与恢复 Set -0
    // 各触发一次写。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/; var sets = 0; var store = -0; Object.defineProperty(r, 'lastIndex', { configurable: true, set(v) { sets += 1; store = v; }, get() { return store; } }); r.exec = function() { return null; }; r[Symbol.search](''); return sets; })()",
    )
    .unwrap();
    assert!(result.as_int() == 2);
}

#[test]
fn symbol_split_species_and_y_flags() {
    let mut vm = Vm::new();
    // species 构造收到补 y 的 flags；捕获组按原值推入（undefined 保留）。
    let result = eval(
        &mut vm,
        "(() => { var flagsSeen = null; var o = { constructor: function() {}, flags: '' }; o.constructor[Symbol.species] = function(_, flags) { flagsSeen = flags; return { exec: function() { return null; }, lastIndex: 0 }; }; var out = RegExp.prototype[Symbol.split].call(o, 'ab'); return flagsSeen + '/' + out.length; })()",
    )
    .unwrap();
    // exec 恒 null：逐单元推进后推整串尾段，结果单元素。
    assert_eq!(to_str(&vm, result), "y/1");
    let result = eval(
        &mut vm,
        "(() => { var o = { constructor: function() {}, flags: 'y' }; o.constructor[Symbol.species] = function(_, flags) { return { exec: function() { return [null, undefined]; }, set lastIndex(v) {}, get lastIndex() { return 1; } }; }; var out = RegExp.prototype[Symbol.split].call(o, 'ab'); return out.length + '/' + String(out[1]); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "3/undefined");
}

#[test]
fn symbol_split_limit_zero_and_to_uint32() {
    let mut vm = Vm::new();
    // lim == 0 直接空数组；limit 经完整 ToNumber（对象转换异常传播）。
    let result = eval(&mut vm, "RegExp.prototype[Symbol.split].call(/b/, 'ab', 0).length").unwrap();
    assert!(result.as_int() == 0);
    let result = eval(
        &mut vm,
        "(() => { var o = { valueOf: function() { throw new RangeError('limit-boom'); } }; try { RegExp.prototype[Symbol.split].call(/b/, 'ab', o); return 'no-throw'; } catch (e) { return e.message; } })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "limit-boom");
}

#[test]
fn symbol_match_all_species_matcher_and_cached_lastindex() {
    let mut vm = Vm::new();
    // matcher 经物种构造；lastIndex 只从 R 读一次并写入 matcher。
    let result = eval(
        &mut vm,
        "(() => { var r = /b/g; r.lastIndex = 2; var ctorCalls = 0; r.constructor[Symbol.species] = function() { ctorCalls += 1; return /b/g; }; var it = r[Symbol.matchAll]('abc'); return ctorCalls + '/' + it.__mal_re__.lastIndex; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "1/2");
    // 非 RegExp this（@@match 布尔 false）：matcher 直接 Construct(%RegExp%, «R, "g"»)。
    let result = eval(
        &mut vm,
        "(() => { var o = { toString: function() { return 'b'; }, flags: 'x', get [Symbol.match]() { return false; } }; var it = RegExp.prototype[Symbol.matchAll].call(o, 'b'); return it.__mal_re__.source + '/' + it.__mal_re__.flags; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "b/g");
}

#[test]
fn symbol_method_name_labels() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "[Symbol.match, Symbol.replace, Symbol.search, Symbol.split, Symbol.matchAll].map(function (s) { return RegExp.prototype[s].name; }).join('|')",
    )
    .unwrap();
    assert_eq!(
        to_str(&vm, result),
        "[Symbol.match]|[Symbol.replace]|[Symbol.search]|[Symbol.split]|[Symbol.matchAll]"
    );
}

#[test]
fn regexp_constructor_regexp_instance_form() {
    let mut vm = Vm::new();
    // (RegExp, flags)：source 取实例 source，flags 参数优先；缺省取实例 flags。
    let result = eval(&mut vm, "new RegExp(/ab/g, 'i').source + '/' + new RegExp(/ab/g, 'i').flags").unwrap();
    assert_eq!(to_str(&vm, result), "ab/i");
    let result = eval(&mut vm, "new RegExp(/ab/g).flags").unwrap();
    assert_eq!(to_str(&vm, result), "g");
}

// --- exec/match 结果面：groups 对象（null 原型、重名键序）与 indices（d 标志）---

#[test]
fn regexp_exec_duplicate_named_groups_first_source_order() {
    // groups 键序按首现源序（y 先于 x）；各分支命中各自命名组取值。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = /(?<y>a)(?<x>a)|(?<x>b)(?<y>b)/.exec('aa'); return Object.keys(m.groups).join(',') + '|' + m.groups.y + m.groups.x; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "y,x|aa");
    let result = eval(
        &mut vm,
        "(() => { var m = /(?<y>a)(?<x>a)|(?<x>b)(?<y>b)/.exec('bb'); return Object.keys(m.groups).join(',') + '|' + m.groups.y + m.groups.x; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "y,x|bb");
}

#[test]
fn regexp_exec_groups_undefined_own_property() {
    // 无命名组时 groups 是自身属性 undefined（非删除），对象不构建。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = /a/.exec('a'); return (m.groups === undefined) + ':' + m.hasOwnProperty('groups') + ':' + (/(a)/.exec('a').groups === undefined); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "true:true:true");
}

#[test]
fn regexp_exec_groups_null_prototype() {
    // groups 对象原型为 null（ObjectCreate(null)），不挂 Object.prototype。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.getPrototypeOf(/a(?<g>b)/.exec('ab').groups) === null").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_constructor_undefined_pattern_is_empty() {
    // undefined 模式按空串编译（非 "undefined"）：串头空命中，index 0。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = new RegExp(undefined).exec('xyz'); return new RegExp(undefined).source + ':' + m[0] + ':' + m.index; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "::0");
}

#[test]
fn regexp_exec_indices_basic_pairs() {
    // d 标志：indices[0] 为完整匹配 [start, end] 码元对，逐捕获组同形；
    // 无 d 标志时无 indices 属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = /(?<g>a)b/d.exec('cab'); return m.indices[0][0] + ',' + m.indices[0][1] + '|' + m.indices[1][0] + ',' + m.indices[1][1]; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "1,3|1,2");
    let result = eval(&mut vm, "/(a)/.exec('a').indices === undefined").unwrap();
    assert!(result.as_bool());
}

#[test]
fn regexp_exec_indices_unmatched_and_groups() {
    // 未匹配捕获组在 indices 中为 undefined；indices.groups 原型 null、值为码元对。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = /a(?<x>b)?/d.exec('a'); return m.indices[1] === undefined ? 'u' : 'n'; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "u");
    let result = eval(
        &mut vm,
        "(() => { var m = /(?<g>a)b/d.exec('cab'); return (Object.getPrototypeOf(m.indices.groups) === null) + ':' + m.indices.groups.g[0] + ',' + m.indices.groups.g[1]; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "true:1,2");
}

#[test]
fn string_match_delegates_exec_result() {
    // 非 global match 交付 exec 结果本体（index/input/未匹配捕获 undefined/
    // groups null 原型）；缺参按空模式串头命中。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var m = 'x0'.match(/(0)(1)?/); return m.index + ':' + m.input + ':' + (m[2] === undefined ? 'u' : 'n') + ':' + (m.groups === undefined ? 'u' : 'n'); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "1:x0:u:u");
    let result = eval(
        &mut vm,
        "(() => { var m = 'x'.match(); return m.length + ':' + m.index + ':' + m[0] + ':' + m.input; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "1:0::x");
}

#[test]
fn string_split_unmatched_capture_undefined() {
    // split 捕获组未匹配时元素为 undefined（非空串）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var p = 'xa(xb)'.split(/x(a)(b)?/); return p.length + ':' + (p[2] === undefined ? 'u' : 'n') + ':' + p[3]; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "4:u:(xb)");
}

#[test]
fn string_match_all_result_indices_and_unmatched() {
    // d 标志下 matchAll 结果挂 indices（与 exec 同面）；未匹配捕获组 undefined。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var it = 'aab'.matchAll(/a(?<g>a)?/dg); var r = it.next().value; return JSON.stringify(r.indices) + ':' + JSON.stringify(r.groups) + ':' + r.index; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "[[0,2],[1,2]]:{\"g\":\"a\"}:0");
    let result = eval(
        &mut vm,
        "(() => { var it = 'x0'.matchAll(/(0)(1)?/g); var r = it.next().value; return r[2] === undefined ? 'u' : 'n'; })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "u");
}

// --- String.replace/replaceAll 替换收尾共享面钉（GetSubstitution 四形态） ---

#[test]
fn string_arm_dollar_expansion_forms() {
    // 字符串臂（捕获表空）：$$/$&/$`/$' 四形态 + $N/$NN 字面回退。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'xay'.replace('a', '$$')").unwrap();
    assert_eq!(to_str(&vm, result), "x$y");
    let result = eval(&mut vm, "'abc'.replace('b', '[$&]')").unwrap();
    assert_eq!(to_str(&vm, result), "a[b]c");
    let result = eval(&mut vm, r"'abc'.replace('b', '$`|$\'')").unwrap();
    assert_eq!(to_str(&vm, result), "aa|cc");
    let result = eval(&mut vm, "'x'.replace('x', '$$$')").unwrap();
    assert_eq!(to_str(&vm, result), "$$");
    let result = eval(&mut vm, "'x'.replace('x', '$1$12')").unwrap();
    assert_eq!(to_str(&vm, result), "$1$12");
}

#[test]
fn string_arm_dollar_lt_forms() {
    // 字符串臂 namedCaptures 恒 undefined：`$<` 只字面输出两码元，其后文本
    // 继续逐码元处理——无 `>` 形、含 `>` 形、名段含 $ 序列形均字面。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'cd'.replace('c', '$<sndcd')").unwrap();
    assert_eq!(to_str(&vm, result), "$<sndcdd");
    let result = eval(&mut vm, "'x'.replace('x', '$<a>')").unwrap();
    assert_eq!(to_str(&vm, result), "$<a>");
    let result = eval(&mut vm, "'x'.replace('x', '$<42$1>')").unwrap();
    assert_eq!(to_str(&vm, result), "$<42$1>");
}

#[test]
fn string_replaceall_dollar_positions() {
    // replaceAll 全量替换 + 空模式逐码元边界（含串尾）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aaa'.replaceAll('a', '$&!')").unwrap();
    assert_eq!(to_str(&vm, result), "a!a!a!");
    let result = eval(&mut vm, "'ab'.replaceAll('', '+')").unwrap();
    assert_eq!(to_str(&vm, result), "+a+b+");
}

#[test]
fn string_replace_regexp_arm_substitution_forms() {
    // RegExp 臂经 String 入口共享 get_substitution_units：$NN 两位回退次位
    // 回落字面、越界整段字面、$< 无 `>` 形续扫、duplicate-names 末次命中。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abcd'.replace(/a(b)(c)(d)/, '$13')").unwrap();
    assert_eq!(to_str(&vm, result), "b3");
    // 两位越界回退一位仍越界（0 组）：整段字面。
    let result = eval(&mut vm, "'x'.replace(/x/, '$33')").unwrap();
    assert_eq!(to_str(&vm, result), "$33");
    let result = eval(&mut vm, "'ab'.replace(/(?<x>a)|(?<x>b)/, '[$<x>]')").unwrap();
    assert_eq!(to_str(&vm, result), "[a]b");
    let result = eval(&mut vm, "'ba'.replace(/(?<x>a)|(?<x>b)/g, '[$<x>]')").unwrap();
    assert_eq!(to_str(&vm, result), "[b][a]");
    let result = eval(&mut vm, r##"'cd'.replace(/(cd)/, '$<sndcd')"##).unwrap();
    assert_eq!(to_str(&vm, result), "$<sndcd");
    let result = eval(&mut vm, r##"'cd'.replace(/(cd)/, '$<snd$<snd')"##).unwrap();
    assert_eq!(to_str(&vm, result), "$<snd$<snd");
    // 未匹配命名组（groups 存在、capture undefined）整段删空至 `>`。
    let result = eval(&mut vm, "'x'.replace(/(?<b>y)?/, '$<b>')").unwrap();
    assert_eq!(to_str(&vm, result), "x");
}

#[test]
fn string_replace_object_searchvalue_delegation() {
    // 对象 searchValue：GetMethod(@@replace) 定义则 Call(matcher, searchValue,
    // «this, replaceValue») 原值返回；真 RegExp 自置 @@replace=undefined 回退
    // 字符串臂（searchString 取 ToString 文本）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { var o = { [Symbol.replace]: function (s, r) { return 'R' + s + r; } }; return 'xy'.replace(o, 'z'); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "Rxyz");
    let result = eval(
        &mut vm,
        "(() => { var r = /./g; Object.defineProperty(r, Symbol.replace, { value: undefined }); return 'aa /./g'.replaceAll(r, 'z'); })()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "aa z");
}

#[test]
fn regexp_replace_results_groups_to_object_and_receiver() {
    // 替换收尾 groups 面：ToObject 装箱（对象直通）、null 抛 TypeError、
    // 原型链数据属性可读、accessor 抛错恢复原异常值。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r##"(() => { var re = { [Symbol.match]: true, [Symbol.replace]: RegExp.prototype[Symbol.replace], flags: '', exec: function () { return { 0: 'A', index: 1, length: 2, groups: { length: 3 } }; } }; return 'xAy'.replace(re, '[$<length>]'); })()"##,
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "x[3]y");
    let result = eval(
        &mut vm,
        r##"(() => { var re = { [Symbol.match]: true, [Symbol.replace]: RegExp.prototype[Symbol.replace], flags: '', exec: function () { return { 0: 'A', index: 1, length: 2, groups: null }; } }; try { 'xAy'.replace(re, '$<x>'); return 'no-throw'; } catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()"##,
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
    let result = eval(
        &mut vm,
        r##"(() => { var g = {}; Object.setPrototypeOf(g, { x: 'c' }); var re = { [Symbol.match]: true, [Symbol.replace]: RegExp.prototype[Symbol.replace], flags: '', exec: function () { return { 0: 'A', index: 1, length: 2, groups: g }; } }; return 'xAy'.replace(re, '[$<x>]'); })()"##,
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "x[c]y");
    let result = eval(
        &mut vm,
        r##"(() => { var re = { [Symbol.match]: true, [Symbol.replace]: RegExp.prototype[Symbol.replace], flags: '', exec: function () { return { 0: 'A', index: 1, length: 2, groups: { get x() { throw 'E'; } } }; } }; try { 'xAy'.replace(re, '$<x>'); return 'no-throw'; } catch (e) { return e; } })()"##,
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "E");
}
