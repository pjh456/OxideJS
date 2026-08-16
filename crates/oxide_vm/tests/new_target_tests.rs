use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn assert_bool(value: JsValue, expected: bool) {
    assert!(value.is_bool(), "expected boolean, got {value:?}");
    assert_eq!(value.as_bool(), expected);
}

#[test]
fn function_call_new_target_is_undefined() {
    // 普通调用不传构造目标：`new.target` 应为 undefined。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return new.target === undefined; } f()").unwrap();
    assert_bool(result, true);
}

#[test]
fn construct_new_target_is_constructor() {
    // `new f()` 以 f 为构造目标：`new.target` 应为 f 本身。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.t = new.target === f; } new f().t").unwrap();
    assert_bool(result, true);
}

#[test]
fn class_constructor_new_target_is_class() {
    // 类构造器内 `new.target` 应为被 new 的类自身。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(){ this.t = new.target; } } new A().t === A").unwrap();
    assert_bool(result, true);
}

#[test]
fn derived_class_new_target_is_derived_constructor() {
    // 子类构造经 super() 链路：基类构造器内 `new.target` 仍是派生类（new 表达式目标）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(){ this.t = new.target; } } class B extends A {} new B().t === B",
    )
    .unwrap();
    assert_bool(result, true);
}

#[test]
fn nested_function_new_target_is_undefined() {
    // 内层普通函数是独立调用帧，不继承外层构造目标的 `new.target`。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function(){ var f = function(){ return new.target; }; return f() === undefined; })()",
    )
    .unwrap();
    assert_bool(result, true);
}

#[test]
fn typeof_and_void_new_target() {
    // unary 表达式组合：构造调用下 `typeof new.target` 为 "function"，普通调用下为 "undefined"。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return typeof new.target; } f()").unwrap();
    assert_eq!(vm.lookup_str(result).expect("typeof should be string"), "undefined");

    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.t = typeof new.target; } new f().t").unwrap();
    assert_eq!(vm.lookup_str(result).expect("typeof should be string"), "function");

    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.u = void new.target; } new f().u").unwrap();
    assert!(result.is_undefined());
}

#[test]
#[ignore = "已知规范缺口：箭头函数 new.target 词法继承未实现（VM 对 arrow 帧写 undefined），修复（闭包捕获）后移除"]
fn arrow_function_inherits_outer_new_target() {
    // 规范要求箭头函数词法继承外层 `new.target`；当前 VM 对 arrow 帧直接写
    // undefined（普通 CALL 路径传 undefined 为 new.target），无词法捕获——
    // 已知规范缺口，登记不修；修复（闭包捕获）后移除 #[ignore]。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return (() => new.target)(); } new f() === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn import_meta_is_explicit_error() {
    // `import.meta` 需模块命名空间对象：显式编译报错（含 "not supported" 供
    // test262 分类 Skip），不留静默错误结果。
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, "import.meta").map_err(|e| format!("Parse error: {:?}", e));
    let program = program.expect("import.meta should parse in module context");
    let err = match Compiler::new().compile(&program) {
        Ok(_) => panic!("import.meta must not compile"),
        Err(e) => e,
    };
    assert!(err.contains("import.meta not yet supported"), "unexpected compile error: {err}");
}

#[test]
fn native_construct_does_not_pollute_new_target() {
    // native 构造（非 spread）不污染调用方 new.target：`new Date()` 后构造器
    // 返回 `new.target` 应仍为外层构造目标 f（255 槽随调用恢复，返回 f 自身）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); return new.target; } (new f()) === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_does_not_pollute_this() {
    // native 构造后同帧 this 保持外层构造的新对象：receiver 槽随调用保存/恢复（254 同修）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); this.t = this instanceof f; } new f().t").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_spread_does_not_pollute_new_target() {
    // spread 构造变体与普通构造一致：`new Date(...[])` 后 new.target 不被污染。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(...[]); return new.target; } (new f()) === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_then_plain_call_new_target_is_undefined() {
    // 普通调用帧内 native 构造后，new.target 仍为 undefined（帧语义不受构造调用干扰）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); return new.target === undefined; } f()").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_keeps_instance_semantics() {
    // 收口 call_function_sync 后 native 构造语义不变：返回真实实例且原型链正确。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "(function(){ var d = new Date(0); return d instanceof Date; })()").unwrap();
    assert_bool(result, true);
}
