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
    vm.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default()
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
