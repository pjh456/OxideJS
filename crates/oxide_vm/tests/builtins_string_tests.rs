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

// ── Plan 01: replace tests ──

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

// ── Plan 02: split tests ──

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

// ── Plan 03: match tests ──

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

// ── Plan 04: matchAll tests ──

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

// ── Plan 05: substring tests ──

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

// ── Plan 06: substr + at tests ──

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

// ── Plan 07: lastIndexOf tests ──

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
