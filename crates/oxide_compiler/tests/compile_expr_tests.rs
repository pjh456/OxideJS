use oxide_compiler::compiler::Compiler;
use oxide_compiler::opcode::{self, OpCode};
use oxide_parser::Allocator;

fn compile_source(source: &str) -> oxide_compiler::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

#[test]
fn compile_simple_expr() {
    let module = compile_source("1 + 2");

    assert!(!module.bytecode.is_empty());
    assert!(module.bytecode.len() >= 4);

    let last = opcode::opcode(*module.bytecode.last().unwrap());
    assert_eq!(last, OpCode::HALT);
}

#[test]
fn compile_constants() {
    let module = compile_source("42");

    assert!(!module.constants.is_empty());
    assert_eq!(
        module.constants[0],
        oxide_compiler::compiler::Constant::Int(42)
    );
}

#[test]
fn compile_negation() {
    let module = compile_source("-5");

    let has_neg = module
        .bytecode
        .iter()
        .any(|&i| opcode::opcode(i) == OpCode::NEG);
    assert!(has_neg);
}

#[test]
fn compile_multiple_stmts() {
    let module = compile_source("1; 2; 3;");

    let load_count = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::LOAD_CONST)
        .count();
    assert!(load_count >= 3);
}

#[test]
fn compile_binary_ops() {
    let tests = [
        ("3 * 4", OpCode::MUL),
        ("10 / 2", OpCode::DIV),
        ("7 % 3", OpCode::MOD),
        ("5 - 2", OpCode::SUB),
    ];

    for (src, expected_op) in tests {
        let module = compile_source(src);
        let has_op = module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == expected_op);
        assert!(has_op, "expected {:?} in '{src}'", expected_op);
    }
}

#[test]
fn compile_ternary_emits_jumps() {
    let module = compile_source("true ? 1 : 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "ternary should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_logical_not() {
    let module = compile_source("!true");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::NOT),
        "!true should emit NOT opcode"
    );
}

#[test]
fn compile_logical_and_simple() {
    let module = compile_source("1 && 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::AND),
        "simple && should emit AND opcode"
    );
}

#[test]
fn compile_logical_or_simple() {
    let module = compile_source("0 || 1");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::OR),
        "simple || should emit OR opcode"
    );
}

#[test]
fn regression_coalesce_consistency() {
    let result = std::panic::catch_unwind(|| {
        compile_source("a ?? b");
    });
    assert!(
        result.is_err(),
        "a ?? b should produce an error (not silently misbehave)"
    );
}

#[test]
fn compile_strict_eq() {
    let module = compile_source("1 === 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
        "1 === 2 should emit STRICT_EQ opcode"
    );
}

#[test]
fn compile_strict_neq() {
    let module = compile_source("1 !== 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_NEQ),
        "1 !== 2 should emit STRICT_NEQ opcode"
    );
}

#[test]
fn compile_unary_plus() {
    let module = compile_source("+'hello'");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::UNARY_PLUS),
        "+'hello' should emit UNARY_PLUS opcode"
    );
}

#[test]
fn compile_typeof_strict_eq() {
    let module = compile_source("typeof 42 === 'number'");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::TYPEOF),
        "should emit TYPEOF opcode"
    );
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
        "should emit STRICT_EQ opcode"
    );
}

#[test]
fn compile_strict_eq_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("1 === 2");
    });
    assert!(result.is_ok(), "1 === 2 should compile without error");
}

#[test]
fn compile_unary_plus_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("+'hello'");
    });
    assert!(result.is_ok(), "+'hello' should compile without error");
}

#[test]
fn compile_compound_add() {
    let module = compile_source("var x=0; x+=1");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::COMPOUND_ADD),
        "x+=1 should emit COMPOUND_ADD opcode"
    );
}

#[test]
fn compile_compound_exp() {
    let module = compile_source("var x=0; x**=2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::COMPOUND_EXP),
        "x**=2 should emit COMPOUND_EXP opcode"
    );
}

#[test]
fn compile_compound_assign_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("x+=1");
    });
    assert!(result.is_ok(), "x+=1 should compile without error");
}

#[test]
fn compile_compound_exp_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("x**=2");
    });
    assert!(result.is_ok(), "x**=2 should compile without error");
}

#[test]
fn compile_inc_pre() {
    let module = compile_source("++x");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::INC_PRE),
        "++x should emit INC_PRE opcode"
    );
}

#[test]
fn compile_inc_post() {
    let module = compile_source("x++");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::INC_POST),
        "x++ should emit INC_POST opcode"
    );
}

#[test]
fn compile_inc_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("x++");
    });
    assert!(result.is_ok(), "x++ should compile without error");
}

#[test]
fn compile_dec_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("x--");
    });
    assert!(result.is_ok(), "x-- should compile without error");
}

#[test]
fn compile_inc_dec_diff_opcodes() {
    let m1 = compile_source("++x");
    let m2 = compile_source("--x");
    let has_inc_pre = m1
        .bytecode
        .iter()
        .any(|&i| opcode::opcode(i) == OpCode::INC_PRE);
    let has_dec_pre = m2
        .bytecode
        .iter()
        .any(|&i| opcode::opcode(i) == OpCode::DEC_PRE);
    assert!(has_inc_pre, "++x should emit INC_PRE");
    assert!(has_dec_pre, "--x should emit DEC_PRE");
}

#[test]
fn compile_member_inc() {
    let module = compile_source("var obj={x:1}; obj.x++");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::MEMBER_INC),
        "obj.x++ should emit MEMBER_INC"
    );
}

#[test]
fn compile_dyn_member_inc() {
    let module = compile_source("var obj={a:3}; var k='a'; obj[k]++");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::DYN_MEMBER_INC),
        "obj[k]++ should emit DYN_MEMBER_INC"
    );
}

#[test]
fn compile_compound_member_add() {
    let module = compile_source("var obj={x:1}; obj.x+=1");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::COMPOUND_MEMBER_ADD),
        "obj.x+=1 should emit COMPOUND_MEMBER_ADD"
    );
}

#[test]
fn compile_compound_member_assign_ok() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var obj={x:1}; obj.x+=1");
    });
    assert!(result.is_ok(), "obj.x+=1 should compile without error");
}

#[test]
fn compile_prefix_member_inc_ok() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var obj={x:1}; ++obj.x");
    });
    assert!(result.is_ok(), "++obj.x should compile without error");
}

#[test]
fn compile_compound_member_exp_ok() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var obj={x:1}; obj.x**=2");
    });
    assert!(result.is_ok(), "obj.x**=2 should compile without error");
}

#[test]
fn compile_this_in_function_ok() {
    let module = compile_source("function f() { return this; }");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_this_member_assign_ok() {
    let module = compile_source("function f() { this.x = 1; }");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_this_member_read_ok() {
    let module = compile_source("function f() { return this.x; }");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_infinity_ok() {
    let module = compile_source("Infinity");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_neg_infinity_ok() {
    let module = compile_source("1 / -Infinity");
    assert!(!module.bytecode.is_empty());
}
