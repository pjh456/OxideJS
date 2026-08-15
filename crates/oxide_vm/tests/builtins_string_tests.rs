use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
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
    // 家族无参守卫对照：search/indexOf 返回 -1，includes/startsWith/endsWith 返回 false，
    // charAt 取首字符——search 与同批方法守卫位置一致、行为家族化。
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
    // 类正则对象（proto 恒等 RegExp.prototype 但无编译正则）：
    // replace/replaceAll 统一按 ToString 文本走字符串路径；默认 toString
    // 不命中源串，故此处断言结果与原串一致（判别性用例见
    // string_replace_regex_proto_subclass_custom_tostring）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "var sp = Object.create(RegExp.prototype); 'aXb'.replace(sp, 'Y')").unwrap();
    assert_eq!(to_str(&vm, s), "aXb");
    let s = eval(&mut vm, "var sp = Object.create(RegExp.prototype); 'aXb'.replaceAll(sp, 'Y')").unwrap();
    assert_eq!(to_str(&vm, s), "aXb");
    let s = eval(
        &mut vm,
        "var sp = Object.create(RegExp.prototype); 'aXb'.replace(sp, function(m){ return 'Z' })",
    )
    .unwrap();
    assert_eq!(to_str(&vm, s), "aXb");
}

#[test]
fn string_replace_regex_proto_subclass_custom_tostring() {
    // 判别性回归：自定义 toString 命中源串的类正则对象，replace/replaceAll
    // 均按 ToString 文本替换（不返回原串、不抛 TypeError），两入口对称。
    let mut vm = Vm::new();
    let s = eval(
        &mut vm,
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replace(sp, 'Y')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, s), "aYb");
    let s = eval(
        &mut vm,
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replaceAll(sp, 'Y')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, s), "aYb");
    let s = eval(
        &mut vm,
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replace(sp, function(){ return 'Z' })",
    )
    .unwrap();
    assert_eq!(to_str(&vm, s), "aZb");
    let s = eval(
        &mut vm,
        "var sp = Object.create(RegExp.prototype); sp.toString = function(){ return 'X' }; 'aXb'.replaceAll(sp, function(){ return 'Z' })",
    )
    .unwrap();
    assert_eq!(to_str(&vm, s), "aZb");
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
    // 类正则对象（proto 恒等 RegExp.prototype 但无编译正则）回退字符串路径，
    // 按 ToString 文本（"[object]"）切分，不落入逐字符切分。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var sp = Object.create(RegExp.prototype); 'aXb'.split(sp)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
    assert_eq!(to_str(&vm, obj.get_prop_at(0)), "aXb");
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
    // 保留所在 astral 字符（孤立代理无法在 UTF-8 表示，见架构限制）。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'\\u{1F600}ab'.slice(1, 3)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}a");
    let s = eval(&mut vm, "'\\u{1F600}ab'.slice(0, 1)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}");
    let s = eval(&mut vm, "'\\u{1F600}'.padStart(3, 'x')").unwrap();
    assert_eq!(to_str(&vm, s), "xx\u{1F600}");
}

#[test]
fn string_char_at_ascii_and_astral() {
    // charAt 产出：ASCII 走单字符缓存，astral 按 UTF-16 单元定位（落在代理对
    // 中间时返回所在 astral 字符，架构限制），越界空串，缺省参数取首字符。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.charAt(1)").unwrap();
    assert_eq!(to_str(&vm, s), "b");
    let s = eval(&mut vm, "'\\u{1F600}ab'.charAt(0)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}");
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
    // 空分隔 split 逐字符产出：ASCII 走单字符缓存，混合 astral 按标量切分。
    let mut vm = Vm::new();
    let s = eval(&mut vm, "'abc'.split('').join('-')").unwrap();
    assert_eq!(to_str(&vm, s), "a-b-c");
    let result = eval(&mut vm, "'\\u{1F600}a'.split('').length").unwrap();
    assert_eq!(result.as_int(), 2);
    let result = eval(&mut vm, "'\\u{1F600}a'.split('')[1]").unwrap();
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
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(2)").unwrap();
    assert_eq!(to_str(&vm, r), "a");
    let r = eval(&mut vm, "'\\u{1F600}ab'.charAt(1)").unwrap();
    assert_eq!(to_str(&vm, r), "\u{1F600}");
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
    // 切片族按 UTF-16 单元计数：边界落在代理对中间时保留所在 astral 字符。
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
    let s = eval(&mut vm, "'\\u{1F600}ab'.substr(1, 2)").unwrap();
    assert_eq!(to_str(&vm, s), "\u{1F600}a");
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
