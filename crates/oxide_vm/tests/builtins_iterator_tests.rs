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

#[test]
fn iterator_proto_chain_and_self_iteration() {
    let mut vm = Vm::new();
    let cases = [
        // 各家族迭代器自迭代恒等（%IteratorPrototype% 的 @@iterator 返回 this）。
        ("var it = [1,2].values(); it[Symbol.iterator]() === it", true),
        ("var it = new Map([['a',1]]).values(); it[Symbol.iterator]() === it", true),
        ("var it = new Set([1]).values(); it[Symbol.iterator]() === it", true),
        ("var it = new Uint8Array([1]).values(); it[Symbol.iterator]() === it", true),
        ("var it = 'abc'[Symbol.iterator](); it[Symbol.iterator]() === it", true),
        ("var it = 'ab'.matchAll(/a/g); it[Symbol.iterator]() === it", true),
        // Array/TA 共享 %ArrayIteratorPrototype%；Map/Set 各自原型内部共享。
        ("Object.getPrototypeOf([].values()) === Object.getPrototypeOf([].entries())", true),
        ("Object.getPrototypeOf(new Uint8Array(0).values()) === Object.getPrototypeOf([].values())", true),
        ("Object.getPrototypeOf(new Map().values()) === Object.getPrototypeOf(new Map().keys())", true),
        ("Object.getPrototypeOf(new Set().values()) === Object.getPrototypeOf(new Set().entries())", true),
        // %IteratorPrototype% 自身可迭代：其 @@iterator 返回 this。
        (
            "var P = Object.getPrototypeOf(Object.getPrototypeOf([].values())); P[Symbol.iterator]() === P",
            true,
        ),
        // 原型链：%ArrayIteratorPrototype% → %IteratorPrototype% → Object.prototype。
        (
            "Object.getPrototypeOf(Object.getPrototypeOf([].values())) === \
             Object.getPrototypeOf(Object.getPrototypeOf(Object.getPrototypeOf([].values())))",
            false,
        ),
        (
            "Object.getPrototypeOf(Object.getPrototypeOf([].values()))[Symbol.iterator] === \
             Object.getPrototypeOf(new Map().values())[Symbol.iterator]",
            true,
        ),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn array_string_symbol_iterator_bound() {
    let mut vm = Vm::new();
    // G4 别名：Array/String 的 @@iterator 属性存在且可迭代。
    let bool_cases = [
        ("[][Symbol.iterator] === [].values", true),
        ("var out = []; for (const c of 'a\\u{1F600}b') out.push(c); out.length === 3", true),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    let str_cases = [
        ("typeof 'abc'[Symbol.iterator]", "function"),
        ("[...'ab'].join('')", "ab"),
        ("Array.from('ab').join('')", "ab"),
        ("[...[1,2,3]].join(',')", "1,2,3"),
        ("Array.from([1,2]).join(',')", "1,2"),
    ];
    for (src, expected) in str_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(to_str(&vm, result), expected, "for {}", src);
    }
    // String 迭代器语义：null/undefined 抛 TypeError，对象走 ToString。
    let err = eval(&mut vm, "try { String.prototype[Symbol.iterator].call(null) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, err), "TypeError");
    let result = eval(&mut vm, "[...String.prototype[Symbol.iterator].call({toString: () => 'xy'})].join('')").unwrap();
    assert_eq!(to_str(&vm, result), "xy");
}

#[test]
fn iterator_proto_object_proto_parent() {
    // 链：values() → %ArrayIteratorPrototype% → %IteratorPrototype% → Object.prototype。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "Object.getPrototypeOf(Object.getPrototypeOf(Object.getPrototypeOf([].values()))) === Object.prototype",
    )
    .unwrap();
    assert!(result.as_bool());
}
