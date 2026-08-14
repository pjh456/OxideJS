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

#[test]
fn iterator_global_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Iterator").unwrap();
    assert_eq!(to_str(&vm, result), "function");
}

#[test]
fn iterator_from_array_returns_values_until_done() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var iter = Iterator.from([1, 2, 3]); \
         var a = iter.next(); var b = iter.next(); var c = iter.next(); var d = iter.next(); \
         a.value === 1 && a.done === false && b.value === 2 && c.value === 3 && d.done === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn iterator_from_string_returns_chars() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var iter = Iterator.from('abc'); iter.next().value").unwrap();
    assert_eq!(to_str(&vm, result), "a");
}

#[test]
fn iterator_from_iterator_forwards_next() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var inner = { next: function() { return { value: 7, done: false }; } }; \
         Iterator.from(inner).next().value",
    )
    .unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn new_iterator_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { new Iterator() } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn iterator_from_string_full_sequence() {
    // 手动 next 逐字符取完全部 ASCII 字符后 done（游标推进路径）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var it = Iterator.from('ab'); \
         var r = []; var x; while (!(x = it.next()).done) r.push(x.value); \
         r.join('')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "ab");
}

#[test]
fn iterator_from_string_astral_scalar() {
    // 字符串迭代按 Unicode 标量推进：astral 字符整体产出（标量语义未动）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var it = Iterator.from('\\u{1F600}'); it.next().value").unwrap();
    assert_eq!(to_str(&vm, result), "\u{1F600}");
}

#[test]
fn iterator_from_string_empty_done() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Iterator.from('').next().done").unwrap();
    assert!(result.as_bool());
}

#[test]
fn for_of_string_chars() {
    // for-of 字符串循环产出逐字符（走字节游标 + 单字符缓存）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var out = []; for (const c of 'abc') out.push(c); out.join('')").unwrap();
    assert_eq!(to_str(&vm, result), "abc");
}
