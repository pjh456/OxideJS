use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

/// 顶层表达式返回 Promise 时，run() 末尾 drain 已执行微任务，读其结算值。
fn settled(vm: &mut Vm, source: &str) -> (bool, JsValue) {
    let result = eval(vm, source).expect("run should succeed");
    assert!(result.is_object(), "expected a promise, got {result:?}");
    let obj = unsafe { &*result.as_js_object_ptr() };
    promise_settled_value(obj).expect("promise should be settled")
}

#[test]
fn resolve_then_drains_chain() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(42).then(v => v * 2)");
    assert!(ok);
    assert_eq!(val.as_double(), 84.0);
}

#[test]
fn chain_orders_fifo() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(1).then(v => v + 1).then(v => v * 10)");
    assert!(ok);
    assert_eq!(val.as_double(), 20.0);
}

#[test]
fn settle_only_once() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "new Promise((res, rej) => { res(1); res(2); rej(3); }).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 1);
}

#[test]
fn throw_in_handler_goes_to_catch() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(1).then(v => { throw 'x' }).catch(e => e)");
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("x"));
}

#[test]
fn thenable_delegation() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve({ then: function(r) { r(7) } }).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 7);
}

#[test]
fn finally_passes_through() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "new Promise(r => { r(10) }).finally(() => {}).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 10);
}

#[test]
fn reject_propagates_through_empty_then() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.reject('e').then(undefined, e => 'caught:' + e)");
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("caught:e"));
}

#[test]
fn ctor_receiver_used_for_capability() {
    let mut vm = Vm::new();
    // Promise.resolve.call(NotPromise) 应构造 NotPromise 并捕获其 executor。
    let result = eval(
        &mut vm,
        "var ef; function NotP(exec) { ef = exec; exec(function(){}, function(){}); } \
         Promise.resolve.call(NotP); ef.length",
    )
    .unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn custom_ctor_failure_is_type_error() {
    let mut vm = Vm::new();
    // executor 未捕获 resolve/reject → TypeError。
    let result = eval(
        &mut vm,
        "function NotP(exec) { exec(function(){}); } \
         try { Promise.resolve.call(NotP); 'no-throw' } catch (e) { e.name }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("TypeError"));
}
