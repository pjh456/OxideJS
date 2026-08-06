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

// 解构参数后跟循环体：计数器必须计入参数解构 prologue，否则循环回跳落到错误 PC。
#[test]
fn param_array_pattern_with_loop_body() {
    assert_eq!(eval("function f([a]){var s=0; while(a>0){s=s+1;a=a-1;} return s;} f([3])"), "3");
}

#[test]
fn param_array_pattern_default_element() {
    assert_eq!(eval("function f([a=5]){return a;} f([])"), "5");
}

// 含默认元素的数组赋值目标后跟循环。
#[test]
fn array_assignment_default_element_then_loop() {
    assert_eq!(eval("var a,c=0; [a=5]=[]; while(c<a){c=c+1;} c"), "5");
}

// 嵌套数组赋值目标元素。
#[test]
fn array_assignment_nested_element() {
    assert_eq!(eval("var a,b; [[a],b]=[[1],2]; a*10+b"), "12");
}

// 解构 catch 参数后跟循环：计数器不得计入模式 catch 绑定未发射的 STORE_VAR。
#[test]
fn destructuring_catch_param_then_loop() {
    assert_eq!(eval("var c=0; try{throw 1;}catch({e}){} while(c<3){c=c+1;} c"), "3");
}

// 回归：for-init 声明无初始化器后跟循环迭代。
#[test]
fn for_init_no_initializer_declarator() {
    assert_eq!(eval("var c=0; for(var i; c<3; c=c+1){} c"), "3");
}

// 标识符元素数组赋值保持正确（简单路径未变）。
#[test]
fn array_assignment_identifier_elements() {
    assert_eq!(eval("var a,b; [a,b]=[7,8]; a*10+b"), "78");
}
