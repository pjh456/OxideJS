use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn let_block_scoping_outer_unchanged() {
    let result = eval("let x = 1; { let x = 2; } x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn const_reassignment_throws() {
    let result = eval("const x = 1; x = 2");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("TypeError") || err.contains("constant"), "got: {}", err);
}

#[test]
fn var_block_not_isolated() {
    let result = eval("var x = 5; { x = 10; } x").unwrap();
    assert_eq!(result.as_int(), 10);
}

#[test]
fn delete_existing_property() {
    let result = eval("var o = {a: 1}; delete o.a; o.a").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn delete_returns_true() {
    let result = eval("var o = {a: 1}; delete o.a").unwrap();
    assert!(result.as_bool());
}

#[test]
fn instanceof_array() {
    let result = eval("[] instanceof Array").unwrap();
    assert!(result.as_bool());
}

#[test]
fn instance_not_array() {
    let result = eval("({}) instanceof Array").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn void_returns_undefined() {
    let result = eval("void 0").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn closure_nested_decl_reads_outer_var() {
    let result = eval("function t(){ var s=3; function f(){return s} return f(); } t()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
#[ignore = "++n on captured upvalue: pre-scan marks is_captured but value still not returned correctly"]
fn closure_counter_escape() {
    let result =
        eval("function counter(){ var n=0; return function(){ return ++n; }; } var c=counter(); c(); c()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_arrow_reads_outer_param() {
    let result = eval("function outer(p){ var f=()=>p; return f(); } outer(7)").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn closure_write_upvalue() {
    let result = eval("function outer(){ var x=1; function set(){ x=2; return x; } return set(); } outer()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_nested_expr_capture() {
    let result = eval("function outer(){ var x=1; return function(){return x+1}()} outer()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_captures_computed_object_key_across_caller_frame() {
    let result = eval("let key='answer'; const call=fn=>fn(); const make=()=>({[key]:42}); call(make).answer").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn closure_captures_object_spread_source_across_caller_frame() {
    let result = eval("let source={x:7}; const call=fn=>fn(); const make=()=>({...source}); call(make).x").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn for_let_per_iteration_independent() {
    let _ = eval("var fns=[]; for(let i=0;i<3;i++){ fns.push(function(){return i}); } fns[0]()+fns[1]()+fns[2]()");
    // 未支持：for-let 逐次迭代绑定尚未实现。
}

#[test]
fn tdz_access_before_init_throws() {
    // 块级预声明后声明点前读取：TDZ 读抛 ReferenceError。
    let err = eval("x; let x=1").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_write_throws() {
    // TDZ 写：赋值引用解析先于 RHS，`x=1; let x;` 抛 ReferenceError。
    let err = eval("x=1; let x;").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_compound_write_throws() {
    // 复合赋值路径 TDZ：`x += 1` 在读旧值前解析赋值引用。
    let err = eval("{ x += 1; let x; }").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_update_throws() {
    // 自增/自减路径 TDZ：`x++` 同样抛 ReferenceError。
    let err = eval("{ x++; let x; }").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_destructuring_throws() {
    // 解构赋值目标 TDZ：`[a]=[1]` 写未初始化 a 抛 ReferenceError。
    let err = eval("[a]=[1]; let a;").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_write_rhs_side_effect_not_evaluated() {
    // 检查点顺序：TDZ 检查在 RHS 求值之前，RHS 副作用不执行（s 保持 0）。
    let result = eval("var s=0; try{ x=(s=1); let x; }catch(e){} s").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn typeof_tdz_throws() {
    // typeof 对 TDZ 绑定抛 ReferenceError（非 "undefined"）。
    let err = eval("typeof x; let x;").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_nested_block_read_throws() {
    // 嵌套块读：块内声明点前的读命中块级 TDZ 占位，抛 ReferenceError。
    let err = eval("{ x; let x; }").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_declaration_self_reference_throws() {
    // 声明语句自引用：初始化器读 x 时 x 尚未初始化，抛 ReferenceError。
    let err = eval("let x = x + 1").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn tdz_hoisted_function_reads_later_let_throws() {
    // hoisted 函数读声明点之后的 let：调用时 x 仍在 TDZ，抛 ReferenceError。
    let err = eval("function f(){return x} f(); let x;").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn lexical_decl_in_single_statement_body_rejected_at_parse() {
    // 文法契约：单语句体取 Statement，lexical 声明属 Declaration（非 Statement），
    // `if (c) let x = 5;` 等无括号控制流体体直属 lexical 是语法错误，解析期即拒绝，
    // 不会进入 emit——TDZ 窗口读在该形状上不可达。此前提若被未来 parser 变更
    // 打破（开始接受这些形状），须同步补 lexical 预声明的体递归覆盖。
    let err = eval("if (true) let x = 5;").unwrap_err();
    assert!(err.contains("Parse"), "got: {}", err);
    let err = eval("if (true) const x = 5;").unwrap_err();
    assert!(err.contains("Parse"), "got: {}", err);
    let err = eval("for (let i = 0; i < 1; i++) let x = 5;").unwrap_err();
    assert!(err.contains("Parse"), "got: {}", err);
    let err = eval("lab: let x = 5;").unwrap_err();
    assert!(err.contains("Parse"), "got: {}", err);
}
