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

/// 数值断言：整数运算保 int，容忍 int/double 两种表示。
fn assert_num(result: JsValue, expected: f64) {
    let actual = if result.is_int() { result.as_int() as f64 } else { result.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

#[test]
fn eval_string_expression_completion_value() {
    let mut vm = Vm::new();
    // 档 1 函数模式：body 包装为语句体匿名函数，隐式返回 undefined，完成值不保留。
    // 档 2 脚本模式（create_dynamic_script）后 `eval('1+2')` 返回 3。
    let result = eval(&mut vm, "eval('1+2')").unwrap();
    assert_eq!(result, JsValue::undefined());
}

#[test]
fn eval_string_number_completion_value() {
    let mut vm = Vm::new();
    // 同档 1 函数模式上限：完成值不保留。
    let result = eval(&mut vm, "eval('42')").unwrap();
    assert_eq!(result, JsValue::undefined());
}

#[test]
fn eval_string_var_declaration_inside() {
    let mut vm = Vm::new();
    // 档 1：eval 内 var 声明只存在于匿名函数作用域，完成值不保留、不泄漏外层。
    let result = eval(&mut vm, "eval('var y = 1; y')").unwrap();
    assert_eq!(result, JsValue::undefined());
}

#[test]
fn eval_non_string_returns_as_is() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval(123)").unwrap();
    assert_num(result, 123.0);
}

#[test]
fn eval_object_identity() {
    let mut vm = Vm::new();
    // 非字符串实参原样返回：同一对象引用。
    let result = eval(&mut vm, "var o = {}; eval(o) === o").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_new_string_not_tostring() {
    let mut vm = Vm::new();
    // new String 是非字符串对象：不 ToString，原样返回同一对象（非原始值）。
    let result = eval(&mut vm, "var s = new String('1+1'); eval(s) === s").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_throw_primitive_rethrown() {
    let mut vm = Vm::new();
    // eval 内 throw 1 重抛原始值 1，可被外层 catch 捕获（非 Error 包装）。
    let result = eval(&mut vm, "try { eval('throw 1') } catch(e) { e }").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn eval_syntax_error_throws() {
    let mut vm = Vm::new();
    // 换行分隔的 `x` 与 `++` 为非法语法，eval 抛 SyntaxError。
    let result = eval(&mut vm, "try { eval('x\\u000A++') } catch(e) { e.name }").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "SyntaxError");
}

#[test]
fn eval_length_is_one() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval.length").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn eval_name_is_eval() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval.name").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "eval");
}

#[test]
fn eval_global_descriptor() {
    let mut vm = Vm::new();
    // 全局 eval 属性描述符：{writable:true, enumerable:false, configurable:true}。
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(this, 'eval'); \
         d.writable && !d.enumerable && d.configurable",
    )
    .unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_new_throws_type_error() {
    let mut vm = Vm::new();
    // eval 不是构造器：new eval() 抛 TypeError。
    let result = eval(&mut vm, "try { new eval() } catch(e) { e.name }").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "TypeError");
}

#[test]
fn eval_no_arg_and_undefined_are_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval() === undefined && eval(undefined) === undefined").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_typeof_is_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof eval").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function");
}

#[test]
fn eval_survives_full_reset_rebuild() {
    let mut vm = Vm::new();
    // 仅 global 脏：full_reset 走 bind_global_functions 重建路径，eval 须保留。
    let g_ptr = vm.session().global_object().as_ptr() as *mut JsObject;
    unsafe { (&mut *g_ptr).bump_generation() };
    vm.full_reset();

    let result = eval(&mut vm, "typeof eval").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function");
    let result = eval(&mut vm, "eval.length === 1").unwrap();
    assert_eq!(result, JsValue::bool(true));
    let result = eval(&mut vm, "eval(123)").unwrap();
    assert_num(result, 123.0);
    assert!(!vm.session().is_dirty_since_snapshot());
}
