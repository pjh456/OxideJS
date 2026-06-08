use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.kernel()
        .string_forge()
        .lookup(val.as_string_index())
        .unwrap_or_default()
}

#[test]
fn string_index_of_found() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "'hello'.indexOf('e')").unwrap();
    assert_eq!(result.as_int(), 1);
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
