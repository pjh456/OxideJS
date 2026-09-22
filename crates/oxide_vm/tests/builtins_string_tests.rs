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

fn assert_num_eq(val: JsValue, expected: f64) {
    let actual = if val.is_int() { val.as_int() as f64 } else { val.as_double() };
    assert_eq!(actual, expected);
}

#[test]
fn string_index_of_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.indexOf('e')").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn string_from_char_code_static() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "String.fromCharCode(65,66,67)").unwrap();
    assert_eq!(to_str(&vm, result), "ABC");
}

#[test]
fn string_index_of_not_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.indexOf('x')").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_includes_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.includes('ell')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn string_includes_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.includes('xyz')").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn string_char_at() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.charAt(1)").unwrap();
    assert_eq!(to_str(&vm, s), "e");
}

#[test]
fn string_char_code_at() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.charCodeAt(0)").unwrap();
    assert_eq!(result.as_int(), 104);
}

#[test]
fn string_concat() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.concat(' world')").unwrap();
    assert_eq!(to_str(&vm, s), "hello world");
}

#[test]
fn string_slice() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.slice(1, 4)").unwrap();
    assert_eq!(to_str(&vm, s), "ell");
}

#[test]
fn string_substring() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substring(0, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "he");
}

#[test]
fn string_to_upper_case() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.toUpperCase()").unwrap();
    assert_eq!(to_str(&vm, s), "HELLO");
}

#[test]
fn string_primitive_length_autoboxes() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.length").unwrap();
    assert_num_eq(result, 3.0);

    let result = eval(&mut vm, "''.length").unwrap();
    assert_num_eq(result, 0.0);

    let result = eval(&mut vm, "'abc'.length + 'de'.length").unwrap();
    assert_num_eq(result, 5.0);
}

#[test]
fn string_length_does_not_break_methods() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.toUpperCase()").unwrap();
    assert_eq!(to_str(&vm, s), "HELLO");
}

#[test]
fn string_to_lower_case() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'HELLO'.toLowerCase()").unwrap();
    assert_eq!(to_str(&vm, s), "hello");
}

#[test]
fn string_trim() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'  hi  '.trim()").unwrap();
    assert_eq!(to_str(&vm, s), "hi");
}

#[test]
fn string_repeat() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.repeat(3)").unwrap();
    assert_eq!(to_str(&vm, s), "abcabcabc");
}

#[test]
fn string_pad_start() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'5'.padStart(4, '0')").unwrap();
    assert_eq!(to_str(&vm, s), "0005");
}

#[test]
fn string_pad_end() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hi'.padEnd(4)").unwrap();
    assert_eq!(to_str(&vm, s), "hi  ");
    // 显式 undefined 填充与缺省同义：回落空格（规范 PadString 步骤）。
    let s = eval(&mut vm, "'hi'.padEnd(4, undefined)").unwrap();
    assert_eq!(to_str(&vm, s), "hi  ");
}

#[test]
fn string_starts_with() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.startsWith('hel')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn string_ends_with() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.endsWith('lo')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn string_split_comma() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a,b,c'.split(',')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(2)), "c");
}

#[test]
fn string_replace_first() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.replace('l', 'L')").unwrap();
    assert_eq!(to_str(&vm, s), "heLlo");
}

#[test]
fn string_search_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.search('ll')").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn string_search_not_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.search('x')").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_search_no_args_returns_minus_one() {
    // 无参调用缺省 searchString：args 仅含 this 槽，守卫返回 -1，不越界 panic。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.search()").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_search_undefined_arg_returns_minus_one() {
    // 显式 undefined 参数：ToString(undefined)="undefined"，在 "abc" 中无匹配返回 -1。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.search(undefined)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_search_empty_pattern_returns_zero() {
    // 空串 pattern：find("") 命中串头，返回 0。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.search('')").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn string_no_arg_guard_family_consistent() {
    // 缺参时 searchString 按 ToString(undefined)="undefined" 参与查找——'abc'
    // 不含 "undefined"，indexOf 得 -1、includes/startsWith/endsWith 得 false
    // （"undefined" 命中的判别断言见 string_missing_arg_searches_undefined）；
    // search 缺参仍走旧守卫返回 -1。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.search()").unwrap();
    assert_eq!(result.as_int(), -1);
    let result = eval(&mut vm, "'abc'.indexOf()").unwrap();
    assert_eq!(result.as_int(), -1);
    let result = eval(&mut vm, "'abc'.includes()").unwrap();
    assert!(!result.as_bool());
    let result = eval(&mut vm, "'abc'.startsWith()").unwrap();
    assert!(!result.as_bool());
    let result = eval(&mut vm, "'abc'.endsWith()").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn string_missing_arg_searches_undefined() {
    // 缺参 searchString 按 ToString(undefined)="undefined" 参与查找（非固定结果短路）。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'undefined'.indexOf()").unwrap();
    assert_eq!(r.as_int(), 0);
    let r = eval(&mut vm, "'abc'.indexOf()").unwrap();
    assert_eq!(r.as_int(), -1);
    let r = eval(&mut vm, "'undefined'.lastIndexOf()").unwrap();
    assert_eq!(r.as_int(), 0);
    let r = eval(&mut vm, "'undefined'.includes()").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "''.includes()").unwrap();
    assert!(!r.as_bool());
    let r = eval(&mut vm, "'undefined'.startsWith()").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'undefined'.endsWith()").unwrap();
    assert!(r.as_bool());
    // 显式传 undefined 与缺参等价（同走 "undefined" 查找）。
    let r = eval(&mut vm, "'undefined'.indexOf(undefined)").unwrap();
    assert_eq!(r.as_int(), 0);
}

#[test]
fn string_ctor_symbol_paths() {
    // 函数调用：Symbol → SymbolDescriptiveString（描述串，不抛）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "String(Symbol('66'))").unwrap();
    assert_eq!(to_str(&vm, s), "Symbol(66)");
    // new 语义：构造器路径走 ToString，Symbol 抛 TypeError。
    let name = eval(&mut vm, "try { new String(Symbol('66')); 'no-throw' } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, name), "TypeError");
}

#[test]
fn string_symbol_argument_throws_type_error() {
    // Symbol 参数经 as_string 抛 TypeError（不静默降级为文本查找）。
    let mut vm = Vm::new();
    for src in [
        "'abc'.indexOf(Symbol('x'))",
        "'abc'.lastIndexOf(Symbol('x'))",
        "'abc'.includes(Symbol('x'))",
        "'abc'.startsWith(Symbol('x'))",
        "'abc'.endsWith(Symbol('x'))",
        "'abc'.replace('a', Symbol('x'))",
        "'abc'.replaceAll('a', Symbol('x'))",
        "'abc'.split(Symbol('x'))",
        "'abc'.search(Symbol('x'))",
        "'abc'.match(Symbol('x'))",
        "'abc'.padStart(5, Symbol('x'))",
        "'abc'.padEnd(5, Symbol('x'))",
        "'abc'.normalize(Symbol('x'))",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{src} 应抛 TypeError，实际: {err}");
    }
}

#[test]
fn string_trim_start() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'  hi  '.trimStart()").unwrap();
    assert_eq!(to_str(&vm, result), "hi  ");
}

#[test]
fn string_trim_end() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'  hi  '.trimEnd()").unwrap();
    assert_eq!(to_str(&vm, result), "  hi");
}

#[test]
fn string_code_point_at_ascii() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'ABC'.codePointAt(1)").unwrap();
    assert_eq!(result.as_int(), 66);
}

#[test]
fn string_normalize_nfc() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.normalize('NFC')").unwrap();
    assert_eq!(to_str(&vm, result), "hello");
}

#[test]
fn string_match_all() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aba'.matchAll(/a/g)").unwrap();
    assert!(result.is_object());
}

#[test]
fn string_replace_all() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aba'.replaceAll('a', 'c')").unwrap();
    assert_eq!(to_str(&vm, result), "cbc");
}

#[test]
fn boxed_string_valueof_and_tostring() {
    let mut vm = Vm::new();
    let v = eval(&mut vm, "new String('abc').valueOf()").unwrap();
    assert!(v.is_string());
    assert_eq!(to_str(&vm, v), "abc");

    let t = eval(&mut vm, "new String('abc').toString()").unwrap();
    assert!(t.is_string());
    assert_eq!(to_str(&vm, t), "abc");
}

#[test]
fn boxed_string_is_object_and_empty_default() {
    let mut vm = Vm::new();
    let ty = eval(&mut vm, "typeof new String('x')").unwrap();
    assert_eq!(to_str(&vm, ty), "object");

    let empty = eval(&mut vm, "new String().valueOf()").unwrap();
    assert_eq!(to_str(&vm, empty), "");
}

#[test]
fn string_call_conversion_stays_primitive() {
    let mut vm = Vm::new();
    let v = eval(&mut vm, "String(123)").unwrap();
    assert!(v.is_string());
    assert_eq!(to_str(&vm, v), "123");
}

// ── replace 测试 ──

#[test]
fn string_replace_non_global_first_only() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'aaa'.replace(/a/, 'b')").unwrap();
    assert_eq!(to_str(&vm, s), "baa");
}

#[test]
fn string_replace_global_all() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'aaa'.replace(/a/g, 'b')").unwrap();
    assert_eq!(to_str(&vm, s), "bbb");
}

#[test]
fn string_replace_function_replacer_match() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.replace(/l/, function(m){return m.toUpperCase()})").unwrap();
    assert_eq!(to_str(&vm, s), "heLlo");
}

#[test]
fn string_replace_function_replacer_offset() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.replace(/b/, function(m,o){return o})").unwrap();
    assert_eq!(to_str(&vm, s), "a1c");
}

#[test]
fn string_replace_string_pattern() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.replace('l', 'L')").unwrap();
    assert_eq!(to_str(&vm, s), "heLlo");
}

#[test]
fn string_replace_string_pattern_only_first() {
    // 字符串 pattern 只替换首个匹配。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'aba'.replace('a', 'c')").unwrap();
    assert_eq!(to_str(&vm, s), "cba");
}

#[test]
fn string_replace_regex_global_all() {
    // 正则 global 全替换，非 global 只替换首个。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.replace(/l/g, 'L')").unwrap();
    assert_eq!(to_str(&vm, s), "heLLo");
    let s = eval(&mut vm, "'hello'.replace(/l/, 'L')").unwrap();
    assert_eq!(to_str(&vm, s), "heLlo");
}

#[test]
fn string_replace_function_replacer_string_pattern() {
    // 函数 replacer + 字符串 pattern：回调参数 (match, position, string)。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'ab'.replace('a', function(m){ return m.toUpperCase() })").unwrap();
    assert_eq!(to_str(&vm, s), "Ab");
    let s = eval(&mut vm, "'abc'.replace('b', function(m, o){ return o })").unwrap();
    assert_eq!(to_str(&vm, s), "a1c");
}

#[test]
fn string_replace_dollar_group_expansion() {
    // `$n` 展开按规范 GetSubstitution：两位数值越界回退一位、仍越界（含 $0）
    // 整段 ref 字面、未参与捕获组为空。预言值对照 V8（node）实测。
    let mut vm = Vm::new();
    let cases = [
        // 两位越界回退一位（次位留字面）。
        ("\"uid=31\".replace(/(uid=)(\\d+)/, \"$11\" + 15)", "uid=115"),
        ("\"x\".replace(/(x)/, \"$12\")", "x2"),
        ("\"xy\".replace(/(x)(y)/, \"$12\")", "x2"),
        // 越界（含 $0）→ ref 字面；无捕获组全字面。
        ("\"xy\".replace(/(x)(y)/, \"$3\")", "$3"),
        ("\"abc\".replace(/b/, \"$0$1\")", "a$0$1c"),
        ("\"abc\".replace(/(b)/, \"$22\")", "a$22c"),
        ("\"x\".replace(/x/, \"$10\")", "$10"),
        // $& / $` 与组引用混排。
        ("\"aaaaa,aaaaa\".replace(/(a+),/, \"$1$&$`$1\")", "aaaaaaaaaa,aaaaaaaaaa"),
        // $$ 折叠与文本内 $ 无关；$ 序列多位数字逐段消费。
        ("\"a$b$c\".replace(/b/, \"$$$$\")", "a$$$$c"),
        ("\"x12y\".replace(/1/, \"$1$12$123\")", "x$1$12$1232y"),
        ("\"x12y\".replace(/(1)/, \"$1$12$123\")", "x1121232y"),
        ("\"x12y\".replace(/(1)(2)/, \"$1$12$123\")", "x112123y"),
        // 组参与但未匹配 → 空。
        ("\"x12y\".replace(/(1)(2)(3)?/, \"$3\")", "xy"),
    ];
    for (src, expected) in cases {
        let s = eval(&mut vm, src).unwrap();
        assert_eq!(to_str(&vm, s), expected, "for {}", src);
    }
}

#[test]
fn string_replace_function_replacer_string_arg() {
    // 回调第 4 参为原字符串：原始 receiver 直接传值（=== 原串），
    // 对象 receiver 传 ToString 内容；字符串/正则 pattern、replace/replaceAll 全覆盖。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.replace('b', function(m, o, str){ return str })").unwrap();
    assert_eq!(to_str(&vm, s), "aabcc");
    let s = eval(&mut vm, "'abc'.replace(/b/, function(m, o, str){ return str })").unwrap();
    assert_eq!(to_str(&vm, s), "aabcc");
    let s = eval(&mut vm, "'abc'.replaceAll('b', function(m, o, str){ return str })").unwrap();
    assert_eq!(to_str(&vm, s), "aabcc");
    // 第 4 参与原始字符串按内容恒等（=== 对字符串按值比较）。
    let s = eval(&mut vm, "var str='abc'; str.replace('b', function(m, o, str2){ return str2 === str })").unwrap();
    assert_eq!(to_str(&vm, s), "atruec");
    // boxed receiver 走对象 ToString 路径，第 4 参为解箱后内容。
    let s = eval(&mut vm, "new String('abc').replace('b', function(m, o, str){ return str })").unwrap();
    assert_eq!(to_str(&vm, s), "aabcc");
    // RegExp.prototype[Symbol.replace] 直调路径（非 String.prototype.replace 快路径）。
    let s = eval(&mut vm, "/b/[Symbol.replace]('abc', function(m, o, str){ return str })").unwrap();
    assert_eq!(to_str(&vm, s), "aabcc");
}

#[test]
fn string_replace_missing_replacement_is_undefined_string() {
    // 缺失 replaceValue 按 ToString(undefined)="undefined" 替换（非空串）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.replace('b')").unwrap();
    assert_eq!(to_str(&vm, s), "aundefinedc");
    let s = eval(&mut vm, "'abc'.replaceAll('b')").unwrap();
    assert_eq!(to_str(&vm, s), "aundefinedc");
    let s = eval(&mut vm, "'abc'.replace('b', undefined)").unwrap();
    assert_eq!(to_str(&vm, s), "aundefinedc");
}

#[test]
fn string_replace_missing_args_replace_undefined_pattern() {
    // 缺失 searchValue 按 "undefined" 文本替换（找不到则原串不变）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'xundefinedy'.replace()").unwrap();
    assert_eq!(to_str(&vm, s), "xundefinedy");
    let s = eval(&mut vm, "'xundefinedy'.replaceAll()").unwrap();
    assert_eq!(to_str(&vm, s), "xundefinedy");
    let s = eval(&mut vm, "'abc'.replace()").unwrap();
    assert_eq!(to_str(&vm, s), "abc");
}

#[test]
fn string_replace_all_nonglobal_regex_throws() {
    // replaceAll 遇非 global 正则抛 TypeError。
    let mut vm = Vm::new();
    for src in [
        "'abc'.replaceAll(/b/, 'X')",
        "'abc'.replaceAll(/b/, function(){ return 'X' })",
        "String.prototype.replaceAll.call('abc', /b/)",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{src} 应抛 TypeError，实际: {err}");
    }
    let s = eval(&mut vm, "'abc'.replaceAll(/b/g, 'X')").unwrap();
    assert_eq!(to_str(&vm, s), "aXc");
}

#[test]
fn string_replace_regex_proto_subclass_fallback() {
    // 类正则对象（proto 恒等 RegExp.prototype 但无编译正则）的默认 toString 是
    // RegExp.prototype.toString，对非 RegExp receiver 抛 TypeError——对象 ToString
    // 异常按规范原样传播，不吞错降级为文本替换；自定义 toString 的判别性用例见
    // string_replace_regex_proto_subclass_custom_tostring。
    let mut vm = Vm::new();
    for src in [
        "var sp = Object.create(RegExp.prototype); 'aXb'.replace(sp, 'Y')",
        "var sp = Object.create(RegExp.prototype); 'aXb'.replaceAll(sp, 'Y')",
        "var sp = Object.create(RegExp.prototype); 'aXb'.replace(sp, function(m){ return 'Z' })",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{src} 应抛 TypeError，实际: {err}");
    }
}

#[test]
fn string_replace_regex_proto_subclass_custom_tostring() {
    // 判别性回归：自定义 toString 命中源串的类正则对象，replace/replaceAll
    // 的 IsRegExp 判定经 GetMethod(@@match) 调默认 @@match——非 RegExp receiver
    // 抛 TypeError 原样传播，不降级为 ToString 文本替换，两入口对称（node 同形）。
    let mut vm = Vm::new();
    for src in [
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replace(sp, 'Y')",
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replaceAll(sp, 'Y')",
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replace(sp, function(){ return 'Z' })",
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replaceAll(sp, function(){ return 'Z' })",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{src} 应抛 TypeError，实际: {err}");
    }
}

#[test]
fn string_replace_boxed_receiver_and_replacement() {
    // boxed receiver 走对象 ToString 路径，boxed replacement 走 ToString 转换。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "new String('abc').replace('b', 'X')").unwrap();
    assert_eq!(to_str(&vm, s), "aXc");
    let s = eval(&mut vm, "'abc'.replace('b', new String('X'))").unwrap();
    assert_eq!(to_str(&vm, s), "aXc");
}

#[test]
fn string_replace_astral_and_empty_pattern() {
    // astral 串走零拷贝快路径不破坏代理对；空 pattern 语义保持。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'\\u{1F600}x\\u{1F600}'.replace('x', 'Y')").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}Y\u{1F600}");
    let s = eval(&mut vm, "'abc'.replace('', '-')").unwrap();
    assert_eq!(to_str(&vm, s), "-abc");
    let s = eval(&mut vm, "'abc'.replaceAll('', '-')").unwrap();
    assert_eq!(to_str(&vm, s), "-a-b-c-");
}

#[test]
fn string_replace_numeric_pattern() {
    // 数字 pattern 经 ToString 文本替换（"1"），replace/replaceAll 均走字符串路径。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'a1b'.replace(1, 'X')").unwrap();
    assert_eq!(to_str(&vm, s), "aXb");
    let s = eval(&mut vm, "'a1b1c'.replaceAll(1, 'X')").unwrap();
    assert_eq!(to_str(&vm, s), "aXbXc");
}

#[test]
fn string_replace_all_boxed_string_pattern() {
    // boxed String pattern 经 ToString 文本替换。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.replaceAll(new String('b'), 'X')").unwrap();
    assert_eq!(to_str(&vm, s), "aXc");
}

#[test]
fn string_replace_empty_receiver() {
    // 空串 receiver + 空 pattern：仅位置 0 一次匹配。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "''.replace('', 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "x");
    let s = eval(&mut vm, "''.replaceAll('', 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "x");
}

// ── split 测试 ──

#[test]
fn string_split_regex_capture_groups() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a1b2c'.split(/(\\d)/)").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 5);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "1");
    assert_eq!(to_str(&vm, obj.get_prop_at(2)), "b");
    assert_eq!(to_str(&vm, obj.get_prop_at(3)), "2");
    assert_eq!(to_str(&vm, obj.get_prop_at(4)), "c");
}

#[test]
fn string_split_regex_capture_with_limit() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a1b2c'.split(/(\\d)/, 3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn string_split_regex_no_capture() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a,b,c'.split(/,/)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "b");
    assert_eq!(to_str(&vm, obj.get_prop_at(2)), "c");
}

#[test]
fn string_split_string_separator() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a,b,c'.split(',')").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn string_split_regex_proto_subclass_fallback() {
    // 类正则对象（proto 恒等 RegExp.prototype 但无编译正则）的默认 toString 抛
    // TypeError（RegExp.prototype.toString 校验 receiver），split 按规范传播异常。
    let mut vm = Vm::new();
    let err = eval(&mut vm, "var sp = Object.create(RegExp.prototype); 'aXb'.split(sp)").unwrap_err();
    assert!(err.contains("TypeError"), "类正则分隔符应抛 TypeError，实际: {err}");
}

#[test]
fn string_split_empty_string_separator() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.split('')").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(2)), "c");

    let empty = eval(&mut vm, "''.split('')").unwrap();
    let obj = unsafe { &*empty.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn string_split_undefined_separator_returns_single() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a-b'.split(undefined)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a-b");
}

#[test]
fn string_split_real_regex_still_regex_path() {
    // 真 RegExp 实例（native_fn 存在）仍走正则切分，含尾部空串。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'ab'.split(/b/)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "");
}

#[test]
fn string_split_regexp_zero_width_skips_separator() {
    // 零宽匹配不推分隔段："hello".split(new RegExp) 逐边界零宽，结果即逐码元。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.split(new RegExp).join('|')").unwrap();
    assert_eq!(to_str(&vm, result), "h|e|l|l|o");
}

#[test]
fn string_split_zero_width_stars_pattern() {
    // /l*/ 每边界零宽匹配：仅非零宽命中处分段，结果 ["h","e","o"]。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.split(/l*/g).join('|')").unwrap();
    assert_eq!(to_str(&vm, result), "h|e|o");
}

#[test]
fn string_split_zero_width_trailing_excluded() {
    // "x".split(/(?:)/) 尾段取自 lastMatchEnd，串尾零宽匹配被循环排除 → ["x"]；
    // 非零宽末匹配尾段照推 → "x".split(/./) 得 ["",""]。
    let mut vm = Vm::new();
    let one = eval(&mut vm, "'x'.split(/(?:)/).join('|')").unwrap();
    assert_eq!(to_str(&vm, one), "x");
    let two = eval(&mut vm, "'x'.split(/./).join('|')").unwrap();
    assert_eq!(to_str(&vm, two), "|");
}

#[test]
fn string_split_empty_input_regexp() {
    // 空串输入：可命中正则单次 exec 得 []，不可命中得 [""]。
    let mut vm = Vm::new();
    let hit = eval(&mut vm, "''.split(/(?:)/).length").unwrap();
    assert_eq!(hit.as_int(), 0);
    let miss = eval(&mut vm, "''.split(/x/).length").unwrap();
    assert_eq!(miss.as_int(), 1);
}

#[test]
fn string_split_limit_to_uint32_wrap() {
    // limit 按 ToUint32 回绕：2^32 → 0 空数组、-1 → 2^32-1 全量、NaN → 0、
    // 缺省与 undefined 均全量。
    let mut vm = Vm::new();
    let wrap = eval(&mut vm, "'a,b,c'.split(',', 2 ** 32).length").unwrap();
    assert_eq!(wrap.as_int(), 0);
    let neg = eval(&mut vm, "'a,b,c'.split(',', -1).length").unwrap();
    assert_eq!(neg.as_int(), 3);
    let nan = eval(&mut vm, "'a,b,c'.split(',', NaN).length").unwrap();
    assert_eq!(nan.as_int(), 0);
    let undef = eval(&mut vm, "'a,b,c'.split(',', undefined).length").unwrap();
    assert_eq!(undef.as_int(), 3);
}

#[test]
fn string_split_limit_object_full_tonumber() {
    // limit 对象经完整 ToNumber：valueOf 直取、转换异常传播。
    let mut vm = Vm::new();
    let via_value_of = eval(&mut vm, "'a|b|c'.split('|', { valueOf: function () { return 2; } }).length").unwrap();
    assert_eq!(via_value_of.as_int(), 2);
    let err = eval(
        &mut vm,
        "'a|b'.split('|', { valueOf: function () { throw new RangeError('limit-boom'); } })",
    )
    .unwrap_err();
    assert!(err.contains("RangeError"), "limit 对象转换异常应传播，实际: {err}");
}

#[test]
fn string_split_order_limit_before_separator_tostring() {
    // 序位：ToUint32(limit) 先于 ToString(separator)——limit 的 valueOf 抛错胜出。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        concat!(
            "'foo'.split(",
            "{ toString: function () { throw new RangeError('sep-boom'); } }, ",
            "{ valueOf: function () { throw new RangeError('limit-boom'); } })"
        ),
    )
    .unwrap_err();
    assert!(err.contains("limit-boom"), "limit 转换应先于 separator 转换，实际: {err}");
}

#[test]
fn string_split_getmethod_dispatch() {
    // 对象分隔符经 GetMethod 派发：Call(splitter, sep, «this, limit»）传原始
    // 寄存器（this 未 ToString、limit 未 ToUint32），结果原值返回。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        concat!(
            "var sep = {}; sep[Symbol.split] = function (s, l) { return [s, l]; }; ",
            "var o = Object.create(String.prototype); ",
            "var r = o.split(sep, 7); ",
            "(r[0] === o) + '|' + r[1]"
        ),
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "true|7");
}

#[test]
fn string_split_getmethod_short_circuits_this_tostring() {
    // GetMethod 派发先于 ToString(this)：this 的 toString 抛错不被触发。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        concat!(
            "var o = new String('x'); ",
            "o.toString = function () { throw new RangeError('this-boom'); }; ",
            "var sep = {}; sep[Symbol.split] = function () { return 42; }; ",
            "o.split(sep)"
        ),
    )
    .unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn string_split_zero_width_missing_capture_not_pushed() {
    // 零宽匹配不推分隔段与捕获组：串内仅零宽命中时缺席捕获组不占位，
    // 结果只剩尾段。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'a'.split(/(b)*/).join('|')").unwrap();
    assert_eq!(to_str(&vm, result), "a");
}

// ── match 测试 ──

#[test]
fn string_match_non_global_captures() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'12-34'.match(/(\\d+)-(\\d+)/)").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "12-34");
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "12");
    assert_eq!(to_str(&vm, obj.get_prop_at(2)), "34");
}

#[test]
fn string_match_no_match_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.match(/x/)").unwrap();
    assert!(result.is_null());
}

#[test]
fn string_match_global_flat() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aba'.match(/a/g)").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    assert_eq!(to_str(&vm, obj.get_prop_at(1)), "a");
}

#[test]
fn string_match_string_pattern() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.match('ll')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "ll");
}

// ── matchAll 测试 ──

#[test]
fn string_match_all_returns_iterator() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'aba'.matchAll(/a/g)").unwrap();
    assert!(result.is_object());
}

#[test]
fn string_match_all_next_returns_match() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var it = 'ab'.matchAll(/a/g); it.next()").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    let done = obj.get_prop_at(1).as_bool();
    assert!(!done);
    assert!(obj.get_prop_at(0).is_object());
}

#[test]
fn string_match_all_next_exhausted() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var it = 'a'.matchAll(/x/g); it.next(); it.next()").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    let done = obj.get_prop_at(1).as_bool();
    assert!(done);
}

#[test]
fn string_match_all_empty_match_advances() {
    // 回归：空匹配（range.start == range.end）后游标不前进会在同位置反复
    // 产出空匹配死循环；每位置应恰好一个空匹配后在有限步内耗尽。
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "var it = 'abc'.matchAll(/(?:)/g); var c = 0; while (!it.next().done) { c++; } c").unwrap();
    assert_num_eq(result, 4.0);
}

#[test]
fn string_match_all_star_empty_advances() {
    // /a*/g 在 'baaab' 上：空匹配（首尾）+ 长匹配混合，须在有限步内耗尽。
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "var it = 'baaab'.matchAll(/a*/g); var c = 0; while (!it.next().done) { c++; } c").unwrap();
    assert_num_eq(result, 4.0);
}

// ── substring 测试 ──

#[test]
fn string_substring_nan_index() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substring(NaN, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "he");
}

#[test]
fn string_substring_negative_index() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substring(-3, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "he");
}

#[test]
fn string_substring_swap() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substring(3, 1)").unwrap();
    assert_eq!(to_str(&vm, s), "el");
}

// ── substr + at 测试 ──

#[test]
fn string_substr_positive() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substr(1, 3)").unwrap();
    assert_eq!(to_str(&vm, s), "ell");
}

#[test]
fn string_substr_negative_start() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substr(-3, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "ll");
}

#[test]
fn string_substr_negative_length() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.substr(1, -1)").unwrap();
    assert_eq!(to_str(&vm, s), "");
}

#[test]
fn string_at_positive() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.at(1)").unwrap();
    assert_eq!(to_str(&vm, s), "e");
}

#[test]
fn string_at_negative() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.at(-1)").unwrap();
    assert_eq!(to_str(&vm, s), "o");
}

#[test]
fn string_at_out_of_range() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'hello'.at(10)").unwrap();
    assert!(s.is_undefined());
}

// ── lastIndexOf 测试 ──

#[test]
fn string_last_index_of_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.lastIndexOf('l')").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn string_last_index_of_not_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.lastIndexOf('x')").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn string_last_index_of_empty_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.lastIndexOf('')").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn string_last_index_of_with_position() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.lastIndexOf('l', 2)").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn string_last_index_of_empty_with_position() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.lastIndexOf('', 3)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn string_pad_start_target_not_exceeding_fast_path() {
    // targetLength 不大于原串长度时快速返回原串。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.padStart(2)").unwrap();
    assert_eq!(to_str(&vm, s), "abc");
    let s = eval(&mut vm, "'abc'.padStart(3, 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "abc");
}

#[test]
fn string_pad_empty_pad_string_fast_path() {
    // padString 为空串时快速返回原串。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'ab'.padStart(4, '')").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
    let s = eval(&mut vm, "'ab'.padEnd(4, '')").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
}

#[test]
fn string_to_well_formed_ascii() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.toWellFormed()").unwrap();
    assert_eq!(to_str(&vm, s), "abc");
    let b = eval(&mut vm, "'abc'.isWellFormed()").unwrap();
    assert!(b.as_bool());
}

#[test]
fn string_normalize_decomposed_and_composed() {
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'\\u00E9'.normalize('NFD')").unwrap();
    assert_eq!(to_str(&vm, s), "e\u{0301}");
    let s = eval(&mut vm, "'e\\u0301'.normalize('NFC')").unwrap();
    assert_eq!(to_str(&vm, s), "\u{00E9}");
}

#[test]
fn boxed_string_receiver_methods() {
    // boxed String 对象作为 receiver 时取内部原始串执行方法。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new String('abc').split('')").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "a");
    let s = eval(&mut vm, "new String('abc').padStart(5, 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "xxabc");
    let s = eval(&mut vm, "new String('abc').toWellFormed()").unwrap();
    assert_eq!(to_str(&vm, s), "abc");
}

#[test]
fn string_methods_null_receiver_throw_type_error() {
    // null receiver 在构造类方法上统一抛 TypeError。
    let mut vm = Vm::new();
    for src in [
        "String.prototype.split.call(null, ',')",
        "String.prototype.padStart.call(null, 5)",
        "String.prototype.slice.call(null)",
        "String.prototype.normalize.call(null)",
        "String.prototype.toWellFormed.call(null)",
        "String.prototype.repeat.call(null, 2)",
        "String.prototype.trim.call(null)",
        "String.prototype.codePointAt.call(null, 0)",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{src} 应抛 TypeError，实际: {err}");
    }
}

#[test]
fn string_slice_astral_character_indices() {
    // UTF-16 单元索引语义：astral 字符占 2 单元，切片边界落在代理对中间时
    // 单元原样保留（单元载荷可承载孤立 surrogate，规格口径）。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var s = '\\u{1F600}ab'.slice(1, 3); s.length === 2 && s.charCodeAt(0) === 0xDE00 && s.charCodeAt(1) === 0x61",
    )
    .unwrap();
    assert!(r.as_bool());
    let r = eval(
        &mut vm,
        "var s = '\\u{1F600}ab'.slice(0, 1); s.length === 1 && s.charCodeAt(0) === 0xD83D",
    )
    .unwrap();
    assert!(r.as_bool());
    let s = eval(&mut vm, "'\\u{1F600}'.padStart(3, 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "xx\u{1F600}");
}

#[test]
fn string_char_at_ascii_and_astral() {
    // charAt 按码元定位：单单元子串（代理对各出 1 单元，规格口径——
    // 规格返回单元子串而非 Unicode 标量），越界空串，缺省参数取首字符。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.charAt(1)").unwrap();
    assert_eq!(to_str(&vm, s), "b");
    let r = eval(
        &mut vm,
        "'\\u{1F600}ab'.charAt(0).length === 1 && '\\u{1F600}ab'.charAt(0).charCodeAt(0) === 0xD83D",
    )
    .unwrap();
    assert!(r.as_bool());
    let r = eval(
        &mut vm,
        "'\\u{1F600}ab'.charAt(1).length === 1 && '\\u{1F600}ab'.charAt(1).charCodeAt(0) === 0xDE00",
    )
    .unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(2) === 'a'").unwrap();
    assert!(r.as_bool());
    let s = eval(&mut vm, "'ab'.charAt(5)").unwrap();
    assert_eq!(to_str(&vm, s), "");
    let s = eval(&mut vm, "'xy'.charAt()").unwrap();
    assert_eq!(to_str(&vm, s), "x");
}

#[test]
fn string_char_at_eq_perm_string() {
    // charAt 产出与字面量比较：内容相等（缓存串与字面量 perm 串）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'abc'.charAt(1) === 'b'").unwrap();
    assert!(result.as_bool());
}

#[test]
fn string_split_empty_separator_chars() {
    // 空分隔 split 逐码元产出：ASCII 走单字符缓存，代理对各出 1 单元元素
    // （规格口径：空分隔按单元切分，孤立 surrogate 各为独立元素）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.split('').join('-')").unwrap();
    assert_eq!(to_str(&vm, s), "a-b-c");
    let result = eval(&mut vm, "'\\u{1F600}a'.split('').length").unwrap();
    assert_eq!(result.as_int(), 3);
    let r = eval(&mut vm, "'\\u{1F600}a'.split('')[1].charCodeAt(0) === 0xDE00").unwrap();
    assert!(r.as_bool());
    let result = eval(&mut vm, "'\\u{1F600}a'.split('')[2]").unwrap();
    assert_eq!(to_str(&vm, result), "a");
}

#[test]
fn string_spread_object_chars() {
    // 对象展开字符串源：索引字符为可枚举自有属性。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const o = {...'ab'}; o['0'] + o['1']").unwrap();
    assert_eq!(to_str(&vm, result), "ab");
}

#[test]
fn string_rest_destructure_chars() {
    // rest 解构字符串源：索引字符收集到 rest 对象。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const {...r} = 'ab'; r['0'] + r['1']").unwrap();
    assert_eq!(to_str(&vm, result), "ab");
}

// ── UTF-16 索引体系回归（astral 字符）──

#[test]
fn string_utf16_length_char_at_code_at() {
    // astral 字符（U+1F600）占 2 个 UTF-16 单元：length/charCodeAt 精确，
    // charAt 按单元定位（代理对中间返回所在 astral 字符）。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\u{1F600}ab'.length").unwrap();
    assert_num_eq(r, 4.0);
    let r = eval(&mut vm, "'\\u{1F600}'.charCodeAt(0)").unwrap();
    assert_eq!(r.as_int(), 0xD83D);
    let r = eval(&mut vm, "'\\u{1F600}'.charCodeAt(1)").unwrap();
    assert_eq!(r.as_int(), 0xDE00);
    let r = eval(&mut vm, "'\\u{1F600}a'.charCodeAt(2)").unwrap();
    assert_eq!(r.as_int(), 0x61);
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(2) === 'a'").unwrap();
    assert!(r.as_bool());
    // charAt 出单单元子串：代理对位置各出 1 单元（高/低 surrogate）。
    let r = eval(
        &mut vm,
        "'\\u{1F600}ab'.charAt(0).length === 1 && '\\u{1F600}ab'.charAt(0).charCodeAt(0) === 0xD83D",
    )
    .unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(1).charCodeAt(0) === 0xDE00").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(4)").unwrap();
    assert_eq!(to_str(&vm, r), "");
}

#[test]
fn string_utf16_at() {
    // at 按 UTF-16 单元定位：正/负索引与越界。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\u{1F600}ab'.at(2)").unwrap();
    assert_eq!(to_str(&vm, r), "a");
    let r = eval(&mut vm, "'\\u{1F600}ab'.at(-1)").unwrap();
    assert_eq!(to_str(&vm, r), "b");
    let r = eval(&mut vm, "'\\u{1F600}ab'.at(-2)").unwrap();
    assert_eq!(to_str(&vm, r), "a");
    let r = eval(&mut vm, "'\\u{1F600}ab'.at(4)").unwrap();
    assert!(r.is_undefined());
}

#[test]
fn string_utf16_slice_substring_substr() {
    // 切片族按 UTF-16 单元计数：边界落在代理对中间时单元原样保留（孤立
    // surrogate 由单元载荷承载，规格口径）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'\\u{1F600}ab'.slice(2)").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
    let s = eval(&mut vm, "'\\u{1F600}ab'.slice(0, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}");
    let s = eval(&mut vm, "'\\u{1F600}ab'.slice(-2)").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
    let s = eval(&mut vm, "'\\u{1F600}ab'.substring(2)").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
    let s = eval(&mut vm, "'\\u{1F600}ab'.substring(0, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}");
    let s = eval(&mut vm, "'\\u{1F600}ab'.substr(2, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "ab");
    let r = eval(
        &mut vm,
        "var s = '\\u{1F600}ab'.substr(1, 2); s.length === 2 && s.charCodeAt(0) === 0xDE00 && s.charCodeAt(1) === 0x61",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_utf16_index_of_last_index_of() {
    // 查找返回值与 position 参数均按 UTF-16 单元计数。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\u{1F600}ab'.indexOf('a')").unwrap();
    assert_eq!(r.as_int(), 2);
    let r = eval(&mut vm, "'\\u{1F600}ab'.indexOf('b', 1)").unwrap();
    assert_eq!(r.as_int(), 3);
    let r = eval(&mut vm, "'\\u{1F600}ab'.indexOf('\\u{1F600}', 1)").unwrap();
    assert_eq!(r.as_int(), -1);
    let r = eval(&mut vm, "'\\u{1F600}ab'.lastIndexOf('a')").unwrap();
    assert_eq!(r.as_int(), 2);
    let r = eval(&mut vm, "'\\u{1F600}ab'.lastIndexOf('b', 2)").unwrap();
    assert_eq!(r.as_int(), -1);
    let r = eval(&mut vm, "'\\u{1F600}ab'.lastIndexOf('')").unwrap();
    assert_eq!(r.as_int(), 4);
}

#[test]
fn string_utf16_includes_starts_ends() {
    // position/endPosition 按 UTF-16 单元；代理对中间的位置无 well-formed 子串可匹配。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\u{1F600}ab'.includes('ab', 2)").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.includes('\\u{1F600}', 1)").unwrap();
    assert!(!r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.startsWith('a', 2)").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.startsWith('\\u{1F600}', 1)").unwrap();
    assert!(!r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.endsWith('ab')").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.endsWith('\\u{1F600}', 1)").unwrap();
    assert!(!r.as_bool());
    let r = eval(&mut vm, "'\\u{1F600}ab'.endsWith('', 1)").unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_utf16_search_offset() {
    // search 返回值按 UTF-16 单元（正则与字符串路径均非字节偏移）。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\u{1F600}ab'.search('b')").unwrap();
    assert_eq!(r.as_int(), 3);
    let r = eval(&mut vm, "'\\u{1F600}ab'.search(/b/)").unwrap();
    assert_eq!(r.as_int(), 3);
    let r = eval(&mut vm, "'\\u{1F600}ab'.search('\\u{1F600}')").unwrap();
    assert_eq!(r.as_int(), 0);
}

#[test]
fn string_utf16_replace_position_callback() {
    // 函数 replacer 的 position 参数按 UTF-16 单元计数（正则与字符串模式）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'\\u{1F600}ab'.replace('b', function(m, o){ return o })").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}a3");
    let s = eval(&mut vm, "'\\u{1F600}ab'.replace(/b/, function(m, o){ return o })").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}a3");
    let s = eval(&mut vm, "'\\u{1F600}ab'.replaceAll('a', function(m, o){ return o })").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}2b");
}

// UTF-16 载荷层回归钉：孤立 surrogate 一等单元（fromCodePoint/fromCharCode 直推单元、
// 字面量/模板 oxc marker 解码、切片/迭代/搜索/键/正则面单元语义）。

#[test]
fn utf16_from_code_point_lone_surrogate() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "String.fromCodePoint(0xD800).length===1 && String.fromCodePoint(0xD800).charCodeAt(0)===0xD800",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_from_char_code_pair_units() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "String.fromCharCode(0xD800,0xDC00).length===2 && String.fromCharCode(0xD800,0xDC00).charCodeAt(1)===0xDC00",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_from_code_point_astral_pair() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "String.fromCodePoint(0x1F600).length===2 && String.fromCodePoint(0x1F600).charCodeAt(0)===0xD83D && String.fromCodePoint(0x1F600).charCodeAt(1)===0xDE00",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_literal_lone_surrogate_emit_decode() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\\ud800'.length===1 && '\\ud800'.charCodeAt(0)===0xD800").unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_template_cooked_lone_surrogate() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "`\\ud800`.length===1").unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_char_at_single_unit_astral() {
    // charAt 按单元取：astral 字符首单元为高代理（非整码点）。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'\u{1D11E}'.charAt(0).charCodeAt(0)===0xD834").unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_slice_substr_single_unit_astral() {
    // slice/substr 单元精确：'\u{1D11E}' = [0xD834, 0xDD1E]。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "'\u{1D11E}'.slice(-1).charCodeAt(0)===0xDD1E && '\u{1D11E}'.substr(1).charCodeAt(0)===0xDD1E && '\u{1D11E}'.slice(0,1).charCodeAt(0)===0xD834",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_iterator_lone_surrogate_units() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ let n=0, ok=true; for (const c of 'a\\ud800') { n++; if (n===2) ok = c.length===1 && c.charCodeAt(0)===0xD800; } return n===2 && ok; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_spread_and_to_object_lone_surrogate() {
    // 迭代/spread 按单元产出；rest 解构的 ToObject 字符串下标读得 1 单元串
    // （非空串）。字符串包装对象索引/length 的构造期物化钉测见
    // boxed_string_exotic_tests.rs。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const arr=[...'a\\ud800']; const o={...'a\\ud800'}; const { ...rest } = 'a\\ud800'; return arr.length===2 && arr[1].length===1 && arr[1].charCodeAt(0)===0xD800 && o[1].length===1 && o[1].charCodeAt(0)===0xD800 && rest[1].length===1 && rest[1].charCodeAt(0)===0xD800; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_regex_surrogate_property_class() {
    // 属性转义须 u 标志（无 u 为规范 SyntaxError，引擎宽容面不在此钉）。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "/\\p{General_Category=Surrogate}/u.test(String.fromCodePoint(0xD800))===true && /\\P{ASCII}/u.test(String.fromCodePoint(0xD800))===true && /\\p{General_Category=Surrogate}/u.test('a')===false",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_is_well_formed_to_well_formed() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "'a\\ud800b'.isWellFormed()===false && 'a\\ud800b'.toWellFormed().length===3 && 'a\\ud800b'.toWellFormed().charCodeAt(1)===0xFFFD",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_index_of_lone_surrogate() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "'a\\ud800\\ud800'.indexOf('\\ud800')===1").unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_property_key_round_trip() {
    let mut vm = Vm::new();
    // 孤立 surrogate 计算键：写入/读取 round-trip，Object.keys 出 1 单元键。
    let r = eval(
        &mut vm,
        "(function(){ const k='\\ud800'; const o={}; o[k]=1; const keys=Object.keys(o); return o[k]===1 && keys.length===1 && keys[0].length===1 && keys[0].charCodeAt(0)===0xD800; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_normalize_astral_identity() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "'\u{1F441}'.normalize('NFD')==='\u{1F441}' && '\u{1F441}'.normalize('NFC').length===2",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_fffd_key_identity_flat_vs_concat() {
    // FFFD 键收口：单字面量（Flat 载荷）与拼接（Cons 载荷，>128 单元）同一逻辑
    // 键在键空间同编码形态——仅一个属性；物化键值与原始串值相等。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const k2 = \"\\uFFFDxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; if (k2.length !== 131) { return false; } const o = {}; o[\"\\uFFFD\" + \"x\".repeat(130)] = 1; o[k2] = 2; const ks = Object.keys(o); return ks.length === 1 && ks[0] === k2 && o[k2] === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn utf16_fffd_hex4_key_materialization() {
    // FFFD+hex4 键物化恒等：键 "\uFFFDd800"（5 单元）经编码形态入键空间，
    // Object.keys 出 5 单元真值（"d800"/"fffd" 4 字符不被当转义吞掉），
    // JSON.stringify/parse 键面复原同值。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const o = {}; o[\"\\uFFFDd800\"] = 1; o[\"\\uFFFDfffd\"] = 2; const ks = Object.keys(o); const rks = Object.keys(JSON.parse(JSON.stringify(o))); return ks[0].length === 5 && ks[1].length === 5 && rks[0] === ks[0] && rks[1] === ks[1]; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn unicode_escape_exactly_four_hex_digits() {
    // \u 恰取 4 位 hex：第 5 位 hex 数字是串内字面字符。
    // "\u10400" = U+1040 + '0'，长度 2，码位 [0x1040, 0x30]。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "\"\\u10400\".length === 2 && \"\\u10400\".charCodeAt(0) === 0x1040 && \"\\u10400\".charCodeAt(1) === 0x30",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn unicode_brace_escape_supplementary_and_lone_surrogate() {
    // 花括号 \u{...} 取任意长度 hex：超平面码点 U+10400 落 UTF-16 单元对
    // （长度 2、codePointAt(0) = 0x10400、与 \uD801\uDC00 形态等值）；
    // \u{D800} 现行 spec 无 surrogate 限制——孤立 surrogate 字面量合法（与
    // V8/node 同接受），长度 1 且与 \ud800 形态等值。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "\"\\u{10400}\".length === 2 && \"\\u{10400}\".codePointAt(0) === 0x10400 \
         && \"\\u{10400}\" === \"\\uD801\\uDC00\" \
         && \"\\u{D800}\".length === 1 && \"\\u{D800}\" === \"\\ud800\"",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_to_locale_lower_case_final_sigma() {
    // Final Sigma 条件映射 21 断言面（special_casing_conditional 15 + U180E 6）：
    // σ/ς 按原始序列双向跳过 Mn/Cf 找最近可见字符判定。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(\"\\u03A3\".toLocaleLowerCase() === \"\\u03C3\" && \
         \"A\\u03A3\".toLocaleLowerCase() === \"a\\u03C2\" && \
         \"\\uD835\\uDCA2\\u03A3\".toLocaleLowerCase() === \"\\uD835\\uDCA2\\u03C2\" && \
         \"A.\\u03A3\".toLocaleLowerCase() === \"a.\\u03C2\" && \
         \"A\\u00AD\\u03A3\".toLocaleLowerCase() === \"a\\u00AD\\u03C2\" && \
         \"A\\uD834\\uDE42\\u03A3\".toLocaleLowerCase() === \"a\\uD834\\uDE42\\u03C2\" && \
         \"\\u0345\\u03A3\".toLocaleLowerCase() === \"\\u0345\\u03C3\" && \
         \"\\u0391\\u0345\\u03A3\".toLocaleLowerCase() === \"\\u03B1\\u0345\\u03C2\" && \
         \"A\\u03A3B\".toLocaleLowerCase() === \"a\\u03C3b\" && \
         \"A\\u03A3\\uD835\\uDCA2\".toLocaleLowerCase() === \"a\\u03C3\\uD835\\uDCA2\" && \
         \"A\\u03A3.b\".toLocaleLowerCase() === \"a\\u03C3.b\" && \
         \"A\\u03A3\\u00ADB\".toLocaleLowerCase() === \"a\\u03C3\\u00ADb\" && \
         \"A\\u03A3\\uD834\\uDE42B\".toLocaleLowerCase() === \"a\\u03C3\\uD834\\uDE42b\" && \
         \"A\\u03A3\\u0345\".toLocaleLowerCase() === \"a\\u03C2\\u0345\" && \
         \"A\\u03A3\\u0345\\u0391\".toLocaleLowerCase() === \"a\\u03C3\\u0345\\u03B1\" && \
         \"A\\u180E\\u03A3\".toLocaleLowerCase() === \"a\\u180E\\u03C2\" && \
         \"A\\u180E\\u03A3B\".toLocaleLowerCase() === \"a\\u180E\\u03C3b\" && \
         \"A\\u03A3\\u180E\".toLocaleLowerCase() === \"a\\u03C2\\u180E\" && \
         \"A\\u03A3\\u180EB\".toLocaleLowerCase() === \"a\\u03C3\\u180Eb\" && \
         \"A\\u180E\\u03A3\\u180E\".toLocaleLowerCase() === \"a\\u180E\\u03C2\\u180E\" && \
         \"A\\u180E\\u03A3\\u180EB\".toLocaleLowerCase() === \"a\\u180E\\u03C3\\u180Eb\")",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_to_locale_case_basic_mapping() {
    // 与 toLowerCase/toUpperCase 同表：ß 小写不变、大写 SS；Σ 大写恒 Σ；
    // 补充平面代理对整体映射。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "\"ß\".toLocaleLowerCase() === \"ß\" && \
         \"ß\".toLocaleUpperCase() === \"SS\" && \
         \"\\u03A3\".toLocaleUpperCase() === \"\\u03A3\" && \
         \"\\u03C3\".toLocaleLowerCase() === \"\\u03C3\" && \
         \"\\u03C3\".toLocaleUpperCase() === \"\\u03A3\" && \
         \"aB\".toLocaleUpperCase() === \"AB\" && \
         \"aB\".toLocaleLowerCase() === \"ab\" && \
          \"\\uD83D\\uDE00\".toLocaleLowerCase() === \"\\uD83D\\uDE00\"",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_locale_compare_canonical_and_order() {
    // NFC 规范等价对全 0（含组合符重排与 Hangul 合成对）；缺省/undefined/
    // "undefined" 三式等价；码元序方向与反对称。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(\"o\\u0308\".localeCompare(\"ö\") === 0 && \
         \"Ç\".localeCompare(\"C\\u0327\") === 0 && \
         \"가\".localeCompare(\"\\u1100\\u1161\") === 0 && \
          \"ô\".localeCompare(\"o\\u0302\") === 0 && \
         \"ṩ\".localeCompare(\"s\\u0323\\u0307\") === 0 && \
         \"a\".localeCompare() === \"a\".localeCompare(undefined) && \
         \"a\".localeCompare() === \"a\".localeCompare(\"undefined\") && \
         \"a\".localeCompare(\"b\") === -1 && \"b\".localeCompare(\"a\") === 1 && \
         \"h\".localeCompare(\"H\") === -\"H\".localeCompare(\"h\") && \
         \"a\".localeCompare(1) === \"a\".localeCompare(\"1\"))",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_locale_methods_type_errors() {
    // null/undefined/symbol 接收者抛 TypeError；localeCompare 第二参
    // Symbol 抛 TypeError。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "((function(){ try { (null).toLocaleLowerCase(); return false; } catch (e) { return e.name === \"TypeError\"; } })() && \
         (function(){ try { (undefined).toLocaleUpperCase(); return false; } catch (e) { return e.name === \"TypeError\"; } })() && \
         (function(){ try { (null).localeCompare(\"a\"); return false; } catch (e) { return e.name === \"TypeError\"; } })() && \
         (function(){ try { \"a\".localeCompare(Symbol(\"s\")); return false; } catch (e) { return e.name === \"TypeError\"; } })())",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_locale_methods_length_and_name() {
    // 绑定描述符面：length 0/0/1、name 与方法名一致。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "String.prototype.toLocaleLowerCase.length === 0 && \
         String.prototype.toLocaleUpperCase.length === 0 && \
         String.prototype.localeCompare.length === 1 && \
         String.prototype.toLocaleLowerCase.name === \"toLocaleLowerCase\" && \
         String.prototype.toLocaleUpperCase.name === \"toLocaleUpperCase\" && \
         String.prototype.localeCompare.name === \"localeCompare\"",
    )
    .unwrap();
    assert!(r.as_bool());
}

/// BigInt 位置参数钉：17 个方法的 ToIntegerOrInfinity 面对 BigInt 抛 TypeError。
#[test]
fn string_methods_bigint_position_type_error() {
    let mut vm = Vm::new();
    for src in [
        "'a'.indexOf('a', 0n)",
        "'abc'.includes('b', 1n)",
        "'abc'.charAt(0n)",
        "'abc'.charCodeAt(0n)",
        "'abc'.lastIndexOf('b', 1n)",
        "'abc'.slice(1n)",
        "'abc'.slice(0, 1n)",
        "'abc'.substring(1n)",
        "'abc'.substring(0, 1n)",
        "'abc'.substr(1n)",
        "'abc'.substr(0, 1n)",
        "'abc'.at(0n)",
        "'ab'.repeat(2n)",
        "'ab'.padStart(3n)",
        "'ab'.padEnd(3n)",
        "'abc'.startsWith('a', 1n)",
        "'abc'.endsWith('c', 2n)",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{}: {}", src, err);
    }
}

/// valueOf 异常透传钉：A 形（indexOf）与 B 形（at）均保留原异常值。
#[test]
fn string_methods_value_of_abort_passes_through() {
    let mut vm = Vm::new();
    let v = eval(&mut vm, "try { 'a'.indexOf('a', {valueOf(){ throw 42 }}) } catch (e) { e }").unwrap();
    assert_eq!(v.as_int(), 42);
    let v = eval(&mut vm, "try { 'abc'.at({valueOf(){ throw 42 }}) } catch (e) { e }").unwrap();
    assert_eq!(v.as_int(), 42);
}

/// repeat count 界判钉：负值与 +Infinity 均抛 RangeError。
#[test]
fn string_repeat_count_range_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "'ab'.repeat(-1)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
    let err = eval(&mut vm, "'ab'.repeat(Infinity)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
}
