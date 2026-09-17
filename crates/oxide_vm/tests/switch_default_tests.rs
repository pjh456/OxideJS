//! switch 无 case 命中时的兜底跳转：跳到 default 体（default 不在源序首位时不能
//! 依赖穿落，否则会错误进入首个 case 体）；无 default 则跳到 switch 末尾。
//!
//! 覆盖：default 末位/中间/首位、有/无命中、穿落、嵌套、空体、for 包裹多迭代、
//! 仅 default、多 case 无命中，以及 case 选择式的严格相等比较。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 求值为字符串的用例：在 VM 存活期内拷出内容，避免借用随 VM 析构失效。
fn eval_str(source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let value = vm.run(&Arc::new(module)).expect("run");
    assert!(value.is_string(), "expected string, got {value:?}");
    // SAFETY: is_string 已确认值为字符串指针，内容在 VM 析构前拷贝为自有 String。
    unsafe { &*value.as_string_ptr() }.as_str().to_string()
}

#[test]
fn switch_default_last_no_match() {
    // default 在末位且无 case 命中时执行 default 体。
    assert_eq!(
        eval_str(r#"let s=""; switch(7){ case 0: s+="a"; break; default: s+="c"; break; } s"#),
        "c"
    );
}

#[test]
fn switch_default_middle_no_match() {
    // default 在中间且无 case 命中时执行 default 体，不穿入首个 case。
    assert_eq!(
        eval_str(r#"let s=""; switch(7){ case 0: s+="a"; break; default: s+="c"; break; case 1: s+="b"; break; } s"#),
        "c"
    );
}

#[test]
fn switch_default_first_no_match() {
    // default 在首位时兜底跳转目标紧邻下一条指令，行为不变。
    assert_eq!(
        eval_str(r#"let s=""; switch(7){ default: s+="c"; break; case 0: s+="a"; break; } s"#),
        "c"
    );
}

#[test]
fn switch_default_only() {
    // 仅 default 时即首个 case 体，穿落正确。
    assert_eq!(eval_str(r#"let s=""; switch(7){ default: s+="c"; break; } s"#), "c");
}

#[test]
fn switch_case_match_skips_default() {
    // 命中 case 时直接跳 case 标签，不经过兜底跳转。
    assert_eq!(
        eval_str(r#"let s=""; switch(0){ case 0: s+="a"; break; default: s+="c"; break; } s"#),
        "a"
    );
}

#[test]
fn switch_case_match_falls_through_default() {
    // 命中 case 后无 break，顺序穿落到 default。
    assert_eq!(eval_str(r#"let s=""; switch(0){ case 0: s+="a"; default: s+="c"; } s"#), "ac");
}

#[test]
fn switch_no_default_no_match() {
    // 无 default 且无命中：跳到 switch 末尾，不执行任何 case 体。
    assert_eq!(eval_str(r#"let s=""; switch(7){ case 0: s+="a"; break; } s"#), "");
}

#[test]
fn switch_multi_case_no_match() {
    // 多 case 均不命中时跳到 default 体。
    assert_eq!(
        eval_str(r#"let s=""; switch(9){ case 1: s+="1"; break; case 2: s+="2"; break; default: s+="d"; break; } s"#),
        "d"
    );
}

#[test]
fn switch_for_wrapped_no_match() {
    // for 包裹多迭代且每轮判别式均不命中：每轮都走兜底跳转转 default。
    assert_eq!(
        eval_str(
            r#"let s=""; for(var i=0;i<3;i++){ switch(i+7){ case 0: s+="a"; break; default: s+="c"; break; } } s"#
        ),
        "ccc"
    );
}

#[test]
fn switch_for_wrapped_mixed_match() {
    // for 包裹下命中与不命中交替：i=0 命中 case，i=1/2 走 default。
    assert_eq!(
        eval_str(r#"let s=""; for(var i=0;i<3;i++){ switch(i){ case 0: s+="a"; break; default: s+="c"; break; } } s"#),
        "acc"
    );
}

#[test]
fn switch_nested_default_no_match() {
    // 嵌套 switch 内层无命中时跳到内层 default，不影响外层。
    assert_eq!(
        eval_str(r#"let s=""; switch(1){ case 1: switch(7){ case 0: s+="a"; break; default: s+="c"; break; } } s"#),
        "c"
    );
}

#[test]
fn switch_empty_body_completion() {
    // 空 CaseBlock 无 case 命中可执行，完成值为 undefined。
    assert!(eval("switch(7){}").unwrap().is_undefined());
}

#[test]
fn switch_completion_default_break() {
    // 无命中跳到 default 体后 break 退出，完成值取自 default 体。
    assert_eq!(eval(r#"1; switch ("b") { case "a": 2; break; default: 3; }"#).unwrap().as_int(), 3);
}

#[test]
fn switch_completion_default_empty_after_case() {
    // default 体为空且不执行时，未写入的结果寄存器读 undefined。
    assert!(eval(r#"7; switch ("b") { case "a": 8; default: }"#).unwrap().is_undefined());
}

#[test]
fn switch_completion_fall_through_default() {
    // 无命中跳到 default 后顺序穿落到其后的 case，完成值为末次执行值。
    assert_eq!(eval(r#"1; switch ("b") { case "a": 10; default: 11; }"#).unwrap().as_int(), 11);
}

#[test]
fn switch_case_strict_equality() {
    // case 选择式按严格相等比较：字符串与数字不匹配，走 default。
    assert_eq!(
        eval_str(r#"let s; switch(1){ case "1": s="loose"; break; default: s="strict"; break; } s"#),
        "strict"
    );
    assert_eq!(
        eval_str(r#"let s; switch(true){ case 1: s="loose"; break; default: s="strict"; break; } s"#),
        "strict"
    );
}
