use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn object_create_and_read() {
    let allocator = Allocator::default();
    let source = "({a:1})";
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let mut vm = Vm::new();
    let obj = vm.run(&module).expect("vm run failed");
    assert!(obj.is_object());
    let obj_ref = unsafe { &*obj.as_js_object_ptr() };
    assert_eq!(obj_ref.prop_count(), 1, "object should have 1 property");
    assert!(obj_ref.get_prop_at(0).is_int());
}

#[test]
fn eval_object_property_read() {
    assert_eq!(eval("({a:1,b:2}).b"), "2");
}

#[test]
fn eval_object_missing_property() {
    assert_eq!(eval("({a:1}).b"), "undefined");
}

#[test]
fn eval_member_inc_post() {
    assert_eq!(eval("var obj={x:1}; obj.x++; obj.x"), "2");
}

#[test]
fn eval_member_inc_post_expr() {
    assert_eq!(eval("var obj={x:1}; obj.x++"), "2");
}

#[test]
fn eval_member_dec() {
    assert_eq!(eval("var obj={x:5}; obj.x--; obj.x"), "4");
}

#[test]
fn eval_member_dec_expr() {
    assert_eq!(eval("var obj={x:5}; obj.x--"), "4");
}

#[test]
fn eval_member_inc_pre() {
    assert_eq!(eval("var obj={x:5}; ++obj.x; obj.x"), "6");
}

#[test]
fn eval_dyn_member_inc() {
    assert_eq!(eval("var obj={a:3}; var k='a'; obj[k]++"), "4");
}

#[test]
fn eval_dyn_member_inc_var() {
    assert_eq!(eval("var obj={a:3}; var k='a'; obj[k]++; obj.a"), "4");
}

#[test]
fn eval_compound_member_add() {
    assert_eq!(eval("var obj={x:5}; obj.x+=3; obj.x"), "8");
}

#[test]
fn eval_compound_member_sub() {
    assert_eq!(eval("var obj={x:10}; obj.x-=2; obj.x"), "8");
}

#[test]
fn eval_compound_member_mul() {
    assert_eq!(eval("var obj={x:2}; obj.x*=3; obj.x"), "6");
}

#[test]
fn eval_compound_member_div() {
    assert_eq!(eval("var obj={x:10}; obj.x/=2; obj.x"), "5");
}

#[test]
fn eval_compound_member_mod() {
    assert_eq!(eval("var obj={x:7}; obj.x%=3; obj.x"), "1");
}

#[test]
fn eval_compound_member_exp() {
    assert_eq!(eval("var obj={x:2}; obj.x**=3; obj.x"), "8");
}

#[test]
fn eval_compound_member_expr_val() {
    assert_eq!(eval("var obj={x:5}; var y=obj.x+=3; y"), "8");
}

#[test]
fn eval_member_multi_inc() {
    assert_eq!(eval("var obj={}; obj.x=0; obj.x++; obj.x++; obj.x"), "2");
}

#[test]
fn eval_numeric_property_key_roundtrip() {
    assert_eq!(eval("var o={}; o[1]=42; o['1']"), "42");
}

#[test]
fn eval_numeric_dynamic_property_key_roundtrip() {
    assert_eq!(eval("var o={}; var k=1; o[k]=42; o['1']"), "42");
}

#[test]
fn eval_numeric_property_key_compound_update() {
    assert_eq!(eval("var o={}; o[1]=2; o[1]++; o['1']"), "3");
}

#[test]
fn eval_in_operator_coerces_numeric_key() {
    assert_eq!(eval("var o={}; o[1]=42; 1 in o"), "true");
}
