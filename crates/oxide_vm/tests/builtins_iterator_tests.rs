use oxide_compiler::compiler::Compiler;
use oxide_types::object::JsObject;
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
        (
            "Object.getPrototypeOf(new Uint8Array(0).values()) === Object.getPrototypeOf([].values())",
            true,
        ),
        (
            "Object.getPrototypeOf(new Map().values()) === Object.getPrototypeOf(new Map().keys())",
            true,
        ),
        (
            "Object.getPrototypeOf(new Set().values()) === Object.getPrototypeOf(new Set().entries())",
            true,
        ),
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

#[test]
fn iterator_function_prototype_property() {
    // Iterator.prototype 属性绑定：=== %IteratorPrototype%（[].values() 原型链中继），
    // 且 %IteratorPrototype% 自身可迭代。
    let mut vm = Vm::new();
    let cases = [
        ("Iterator.prototype !== undefined", true),
        ("Object.getPrototypeOf(Object.getPrototypeOf([].values())) === Iterator.prototype", true),
        (
            "Object.getPrototypeOf(Object.getPrototypeOf(new Map().values())) === Iterator.prototype",
            true,
        ),
        ("Iterator.prototype[Symbol.iterator]() === Iterator.prototype", true),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn string_iterator_proto_layer() {
    // String 迭代器挂 %StringIteratorPrototype% 中继层：与 Array 迭代器原型不同，
    // 且经链到 %IteratorPrototype%（Iterator.prototype）。
    let mut vm = Vm::new();
    let cases = [
        (
            "Object.getPrototypeOf(Object.getPrototypeOf('a'[Symbol.iterator]())) === Iterator.prototype",
            true,
        ),
        (
            "Object.getPrototypeOf('a'[Symbol.iterator]()) !== Object.getPrototypeOf([].values())",
            true,
        ),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    let out = eval(&mut vm, "[...'abc'].join('')").unwrap();
    assert_eq!(to_str(&vm, out), "abc");
}

#[test]
fn iterator_next_on_prototypes() {
    // next 挂集合迭代器原型（%ArrayIteratorPrototype%.next 等），wrapper 无 own next。
    let mut vm = Vm::new();
    let cases = [
        ("typeof Object.getPrototypeOf([].values()).next === 'function'", true),
        ("Object.getPrototypeOf([].values()).hasOwnProperty('next')", true),
        ("[].values().hasOwnProperty('next') === false", true),
        ("typeof Object.getPrototypeOf(new Map().values()).next === 'function'", true),
        ("new Map().values().hasOwnProperty('next') === false", true),
        ("typeof Object.getPrototypeOf(new Set().values()).next === 'function'", true),
        ("new Set().values().hasOwnProperty('next') === false", true),
        ("typeof Object.getPrototypeOf(new Uint8Array([1]).values()).next === 'function'", true),
        ("new Uint8Array([1]).values().hasOwnProperty('next') === false", true),
        ("typeof Object.getPrototypeOf('ab'.matchAll(/a/g)).next === 'function'", true),
        ("'ab'.matchAll(/a/g).hasOwnProperty('next') === false", true),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_proto_next_consumption_unchanged() {
    // 行为回归：next 移上原型后各家族消费路径不变（迭代协议经原型链解析）。
    let mut vm = Vm::new();
    let str_cases = [
        ("[...[1,2,3]].join(',')", "1,2,3"),
        ("var it = [1,2].values(); it.next().value + ',' + it.next().value", "1,2"),
        ("var it = ['x','y'].entries(); it.next().value.join(':')", "0:x"),
        ("[...new Map([['a',1]])][0].join(':')", "a:1"),
        ("var mk = new Map([['k',9]]); mk.keys().next().value", "k"),
        ("var se = new Set([7]); se.entries().next().value.join(':')", "7:7"),
        ("new Uint8Array([9]).entries().next().value.join(',')", "0,9"),
        ("[...'ab'].join('')", "ab"),
        ("'a\\u{1F600}b'[Symbol.iterator]().next().value", "a"),
        ("var mm = 'ab'.matchAll(/b/g); mm.next().value[0]", "b"),
    ];
    for (src, expected) in str_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(to_str(&vm, result), expected, "for {}", src);
    }
    // 数值结果（含 int 0）用恒等比较断言，避免格式化路径差异。
    let num_cases = [
        ("[10,20].keys().next().value === 0", true),
        ("[1,2].values().next().value === 1", true),
        ("new Map([['a',1]]).values().next().value === 1", true),
        ("new Set([5,6]).keys().next().value === 5", true),
        ("new Uint8Array([5]).values().next().value === 5", true),
        ("new Uint8Array([7]).entries().next().value[1] === 7", true),
    ];
    for (src, expected) in num_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    // TA 迭代耗尽后恒 done（数组路径 target 置 undefined 防复活）。
    let done = eval(&mut vm, "var it = new Uint8Array([1]).values(); it.next(); it.next().done").unwrap();
    assert!(done.as_bool());
}

#[test]
fn iterator_function_name_length() {
    let mut vm = Vm::new();
    let bool_cases = [
        ("Iterator.name === 'Iterator'", true),
        ("Iterator.length === 0", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'name').writable === false", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'name').enumerable === false", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'name').configurable === true", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'length').writable === false", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'length').enumerable === false", true),
        ("Object.getOwnPropertyDescriptor(Iterator, 'length').configurable === true", true),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_prototype_constructor_accessor() {
    let mut vm = Vm::new();
    let bool_cases = [
        // getter 动态读 global 上的 Iterator 构造器。
        ("Iterator.prototype.constructor === Iterator", true),
        (
            "typeof Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor').get === 'function'",
            true,
        ),
        (
            "typeof Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor').set === 'function'",
            true,
        ),
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor').enumerable === false",
            true,
        ),
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor').configurable === true",
            true,
        ),
        // getter 无参调用（this=undefined）仍返回 Iterator。
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor').get.call() === Iterator",
            true,
        ),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_prototype_constructor_setter_semantics() {
    let mut vm = Vm::new();
    // home 对象赋值与原始值 this 抛 TypeError（SetterThatIgnoresPrototypeProperties）。
    let throws = [
        (
            "try { Iterator.prototype.constructor = 'x'; false } catch (e) { e instanceof TypeError }",
            true,
        ),
        (
            "try { var d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor'); \
             d.set.call(undefined, 'x'); false } catch (e) { e instanceof TypeError }",
            true,
        ),
        (
            "try { var d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor'); \
             d.set.call(null, 'x'); false } catch (e) { e instanceof TypeError }",
            true,
        ),
        (
            "try { var d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor'); \
             d.set.call(true, 'x'); false } catch (e) { e instanceof TypeError }",
            true,
        ),
    ];
    for (src, expected) in throws {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    // 非 home 对象：无 own 属性建 own，有 own 属性走 Set 覆盖。
    let ok_cases = [
        (
            "var o = {}; var d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor'); \
             d.set.call(o, 42); o.constructor === 42 && o.hasOwnProperty('constructor')",
            true,
        ),
        (
            "var o = { constructor: 1 }; var d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor'); \
             d.set.call(o, 2); o.constructor === 2",
            true,
        ),
        // home 不受污染。
        ("Iterator.prototype.constructor === Iterator", true),
    ];
    for (src, expected) in ok_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_prototype_to_string_tag_accessor() {
    let mut vm = Vm::new();
    let bool_cases = [
        ("Iterator.prototype[Symbol.toStringTag] === 'Iterator'", true),
        (
            "typeof Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag).get === 'function'",
            true,
        ),
        (
            "typeof Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag).set === 'function'",
            true,
        ),
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag).enumerable === false",
            true,
        ),
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag).configurable === true",
            true,
        ),
        (
            "Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag).get.call() === 'Iterator'",
            true,
        ),
        // home 赋值抛 TypeError。
        (
            "try { Iterator.prototype[Symbol.toStringTag] = 'x'; false } catch (e) { e instanceof TypeError }",
            true,
        ),
        // 普通对象 setter 建 own Symbol.toStringTag。
        (
            "var o = {}; var d = Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag); \
             d.set.call(o, 'tag'); o[Symbol.toStringTag] === 'tag'",
            true,
        ),
        // 原型继承场景：非 home 子对象赋值落到自身 own 属性（不污染祖先）。
        (
            "var p = Object.create(Iterator.prototype); p[Symbol.toStringTag] = 'sub'; \
             p[Symbol.toStringTag] === 'sub' && Iterator.prototype[Symbol.toStringTag] === 'Iterator'",
            true,
        ),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn symbol_dispose_and_async_dispose_registered() {
    let mut vm = Vm::new();
    // well-known symbol 以空对象表示，typeof 为 object；两符号互不相同。
    let bool_cases = [
        ("typeof Symbol.dispose === 'object'", true),
        ("typeof Symbol.asyncDispose === 'object'", true),
        ("Symbol.dispose !== Symbol.asyncDispose", true),
        ("Symbol.dispose !== Symbol.iterator", true),
        // 键可作属性名使用（well-known 键映射一致）。
        ("({ [Symbol.dispose]: 1 })[Symbol.dispose] === 1", true),
        // getOwnPropertySymbols 能反解出同一符号对象。
        ("Object.getOwnPropertySymbols({ [Symbol.dispose]: 1 })[0] === Symbol.dispose", true),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    // 描述性名称经 String 构造器反射（well_known_symbol_name 表）。
    let desc = eval(&mut vm, "String(Symbol.dispose)").unwrap();
    assert_eq!(to_str(&vm, desc), "Symbol(Symbol.dispose)");
}

#[test]
fn iterator_prototype_symbol_dispose() {
    let mut vm = Vm::new();
    // @@dispose 调用 this 的 return 方法并返回 undefined。
    let bool_cases = [
        ("typeof Iterator.prototype[Symbol.dispose] === 'function'", true),
        ("Iterator.prototype[Symbol.dispose].length === 0", true),
        ("Iterator.prototype[Symbol.dispose].name === '[Symbol.dispose]'", true),
        (
            "var called = false; var it = { return: function() { called = true; return { done: true }; } }; \
             var r = Iterator.prototype[Symbol.dispose].call(it); r === undefined && called === true",
            true,
        ),
        // 无 return 方法：GetMethod 返回 undefined，跳过调用。
        ("Iterator.prototype[Symbol.dispose].call({ next: function() {} }) === undefined", true),
        // return 返回任意值不影响 @@dispose 的 undefined 返回值。
        (
            "var it = { return: function() { return 42; } }; \
             Iterator.prototype[Symbol.dispose].call(it) === undefined",
            true,
        ),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    // return 抛错经 @@dispose 透传（原值，不二次包装）。
    let thrown = eval(
        &mut vm,
        "try { var it = { return: function() { throw new RangeError('boom'); } }; \
         Iterator.prototype[Symbol.dispose].call(it); false } catch (e) { e instanceof RangeError }",
    )
    .unwrap();
    assert!(thrown.as_bool());
}

#[test]
fn iterator_helper_prototype_links_to_iterator_proto() {
    let vm = Vm::new();
    // %IteratorHelperPrototype% 已建且链到 %IteratorPrototype%（链：
    // helper → %IteratorPrototype% → Object.prototype）。
    let helper_ptr = vm.session().builtin_world().iterator_helper_proto.as_ptr() as *mut JsObject;
    let helper_proto_val = unsafe { &*helper_ptr }.proto();
    let iter_ptr = vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    assert!(helper_proto_val.is_object());
    assert!(std::ptr::eq(helper_proto_val.as_js_object_ptr(), iter_ptr));
}

#[test]
fn iterator_constructor_plain_call_throws() {
    let mut vm = Vm::new();
    // 普通调用 `Iterator()` 抛 TypeError（emit 以 undefined 作 this）。
    let bool_cases = [
        ("try { Iterator(); false } catch (e) { e instanceof TypeError }", true),
        // call/apply 显式传对象 this 顶层调用同样抛（newTarget 槽为 undefined）。
        ("try { Iterator.call({}); false } catch (e) { e instanceof TypeError }", true),
        ("try { Iterator.apply({}, []); false } catch (e) { e instanceof TypeError }", true),
        // 以 %IteratorPrototype% 为原型的对象顶层调用也抛（home 形态误判保护）。
        (
            "try { Iterator.call(Object.create(Iterator.prototype)); false } \
             catch (e) { e instanceof TypeError }",
            true,
        ),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_constructor_subclassable() {
    let mut vm = Vm::new();
    // `class X extends Iterator {}` 的 super() 经放行路径返回 undefined，
    // 实例原型按 newTarget 设为 X.prototype。
    let bool_cases = [
        (
            "class TestIterator extends Iterator {} \
             var it = new TestIterator(); \
             it instanceof TestIterator && it instanceof Iterator",
            true,
        ),
        (
            "class TestIterator extends Iterator {} \
             Object.getPrototypeOf(new TestIterator()) === TestIterator.prototype",
            true,
        ),
        (
            "class TestIterator extends Iterator {} \
             Object.getPrototypeOf(TestIterator.prototype) === Iterator.prototype",
            true,
        ),
        // 显式 constructor 内 super() 与隐式等价。
        (
            "class TestIterator extends Iterator { constructor() { super(); } } \
             new TestIterator() instanceof Iterator",
            true,
        ),
        // 多层继承：newTarget 跨层保持为最外层类。
        (
            "class A extends Iterator {} class B extends A {} \
             var b = new B(); \
             b instanceof B && b instanceof A && b instanceof Iterator",
            true,
        ),
        // 构造器 name 不受影响。
        ("class TestIterator extends Iterator {} TestIterator.name === 'TestIterator'", true),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_constructor_subclass_still_rejects_new_after_reset() {
    let mut vm = Vm::new();
    // dirty reset 后构造器经 bind_iterator_global 重绑，本体与 subclass 路径不变。
    unsafe { &mut *(vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject) }.bump_generation();
    vm.full_reset();
    let bool_cases = [
        ("try { new Iterator(); false } catch (e) { e instanceof TypeError }", true),
        (
            "class TestIterator extends Iterator {} \
             new TestIterator() instanceof Iterator",
            true,
        ),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn iterator_prototype_rebound_after_full_reset() {
    let mut vm = Vm::new();
    // 用户改写 %IteratorPrototype%：object 家族世代递增（global 未动）。
    let old_iter_proto = vm.session().builtin_world().iterator_proto.as_ptr();
    unsafe { &mut *(old_iter_proto as *mut JsObject) }.bump_generation();

    vm.full_reset();

    // %IteratorPrototype% 重建后访问器 / @@dispose 重绑正确，构造器 identity 保留。
    let cases = [
        ("Iterator.prototype.constructor === Iterator", true),
        ("Iterator.prototype[Symbol.toStringTag] === 'Iterator'", true),
        ("typeof Iterator.prototype[Symbol.dispose] === 'function'", true),
        ("Iterator.name === 'Iterator'", true),
        ("Iterator.length === 0", true),
        ("typeof Symbol.dispose === 'object'", true),
        ("typeof Symbol.asyncDispose === 'object'", true),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
    // helper 原型随 object 家族重建并链到新 %IteratorPrototype%。
    let helper_ptr = vm.session().builtin_world().iterator_helper_proto.as_ptr() as *mut JsObject;
    let helper_proto_val = unsafe { &*helper_ptr }.proto();
    let new_iter_ptr = vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    assert!(std::ptr::eq(helper_proto_val.as_js_object_ptr(), new_iter_ptr));
    // 迭代器家族整体功能完好。
    let r = eval(
        &mut vm,
        "Iterator.prototype[Symbol.dispose].call({return: function(){return {done:true}}})",
    )
    .unwrap();
    assert!(r.is_undefined());
}
