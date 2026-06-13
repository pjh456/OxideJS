use oxide_compiler::compiler::Compiler;
use oxide_compiler::opcode::{self, OpCode};
use oxide_parser::Allocator;

fn compile_source(source: &str) -> oxide_compiler::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

fn compile_source_err(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    match compiler.compile(&program) {
        Ok(_) => panic!("compile should fail"),
        Err(err) => err,
    }
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
fn unsupported_logical_assignment_returns_compile_error() {
    let err = compile_source_err("let x = 0; x ||= 1;");
    assert!(err.contains("compound assignment operator"));
    assert!(err.contains("not supported"));
}

#[test]
fn unsupported_member_logical_assignment_returns_compile_error() {
    let err = compile_source_err("let obj = { x: 0 }; obj.x ||= 1;");
    assert!(err.contains("compound assignment operator"));
    assert!(err.contains("not supported"));
}

#[test]
fn compile_constants() {
    let module = compile_source("42");

    assert!(!module.constants.is_empty());
    assert_eq!(module.constants[0], oxide_compiler::compiler::Constant::Int(42));
}

#[test]
fn compile_dedups_identical_string_constants() {
    let module = compile_source("var a = 'hello'; var b = 'hello';");
    let count = module
        .constants
        .iter()
        .filter(|c| matches!(c, oxide_compiler::compiler::Constant::String(s) if s == "hello"))
        .count();
    assert_eq!(count, 1, "identical string constants should be interned once per module");
}

#[test]
fn compile_negation() {
    let module = compile_source("-5");

    let has_neg = module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NEG);
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
fn compile_object_literal_getter_emits_define_accessor() {
    let module = compile_source("var o = { get x() { return 1; } };");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEFINE_ACCESSOR),
        "object literal getter should emit DEFINE_ACCESSOR"
    );
}

#[test]
fn compile_object_literal_setter_emits_define_accessor() {
    let module = compile_source("var o = { set x(v) { this.y = v; } };");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEFINE_ACCESSOR),
        "object literal setter should emit DEFINE_ACCESSOR"
    );
}

#[test]
fn compile_binary_ops() {
    let tests = [
        ("3 * 4", OpCode::MUL),
        ("10 / 2", OpCode::DIV),
        ("7 % 3", OpCode::MOD),
        ("5 - 2", OpCode::SUB),
        ("5 & 3", OpCode::BIT_AND),
        ("5 | 2", OpCode::BIT_OR),
        ("5 ^ 1", OpCode::BIT_XOR),
        ("1 << 3", OpCode::SHL),
        ("-8 >> 1", OpCode::SHR),
        ("-1 >>> 0", OpCode::USHR),
    ];

    for (src, expected_op) in tests {
        let module = compile_source(src);
        let has_op = module.bytecode.iter().any(|&i| opcode::opcode(i) == expected_op);
        assert!(has_op, "expected {:?} in '{src}'", expected_op);
    }
}

#[test]
fn compile_bitwise_not() {
    let module = compile_source("~0");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::BIT_NOT),
        "~0 should emit BIT_NOT opcode"
    );
}

#[test]
fn compile_ternary_emits_jumps() {
    let module = compile_source("true ? 1 : 2");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "ternary should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_logical_not() {
    let module = compile_source("!true");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NOT),
        "!true should emit NOT opcode"
    );
}

#[test]
fn compile_logical_and_simple() {
    let module = compile_source("1 && 2");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::AND),
        "simple && should emit AND opcode"
    );
}

#[test]
fn compile_logical_or_simple() {
    let module = compile_source("0 || 1");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::OR),
        "simple || should emit OR opcode"
    );
}

#[test]
fn regression_coalesce_consistency() {
    let result = std::panic::catch_unwind(|| {
        compile_source("a ?? b");
    });
    assert!(result.is_err(), "a ?? b should produce an error (not silently misbehave)");
}

#[test]
fn compile_strict_eq() {
    let module = compile_source("1 === 2");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
        "1 === 2 should emit STRICT_EQ opcode"
    );
}

#[test]
fn compile_strict_neq() {
    let module = compile_source("1 !== 2");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::STRICT_NEQ),
        "1 !== 2 should emit STRICT_NEQ opcode"
    );
}

#[test]
fn compile_unary_plus() {
    let module = compile_source("+'hello'");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::UNARY_PLUS),
        "+'hello' should emit UNARY_PLUS opcode"
    );
}

#[test]
fn compile_typeof_strict_eq() {
    let module = compile_source("typeof 42 === 'number'");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::TYPEOF),
        "should emit TYPEOF opcode"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
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
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::COMPOUND_ADD),
        "x+=1 should emit COMPOUND_ADD opcode"
    );
}

#[test]
fn compile_compound_exp() {
    let module = compile_source("var x=0; x**=2");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::COMPOUND_EXP),
        "x**=2 should emit COMPOUND_EXP opcode"
    );
}

#[test]
fn compile_compound_bitwise_ops() {
    let tests = [
        ("var x=5; x&=3", OpCode::COMPOUND_AND),
        ("var x=5; x|=2", OpCode::COMPOUND_OR),
        ("var x=5; x^=1", OpCode::COMPOUND_XOR),
        ("var x=1; x<<=5", OpCode::COMPOUND_SHL),
        ("var x=-8; x>>=1", OpCode::COMPOUND_SHR),
        ("var x=-1; x>>>=0", OpCode::COMPOUND_USHR),
    ];

    for (src, expected_op) in tests {
        let module = compile_source(src);
        assert!(
            module.bytecode.iter().any(|&i| opcode::opcode(i) == expected_op),
            "expected {:?} in '{src}'",
            expected_op
        );
    }
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
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::INC_PRE),
        "++x should emit INC_PRE opcode"
    );
}

#[test]
fn compile_inc_post() {
    let module = compile_source("x++");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::INC_POST),
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
    let has_inc_pre = m1.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::INC_PRE);
    let has_dec_pre = m2.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEC_PRE);
    assert!(has_inc_pre, "++x should emit INC_PRE");
    assert!(has_dec_pre, "--x should emit DEC_PRE");
}

#[test]
fn compile_member_inc() {
    let module = compile_source("var obj={x:1}; obj.x++");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::MEMBER_INC),
        "obj.x++ should emit MEMBER_INC"
    );
}

#[test]
fn compile_dyn_member_inc() {
    let module = compile_source("var obj={a:3}; var k='a'; obj[k]++");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DYN_MEMBER_INC),
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
fn compile_global_this_ok() {
    let module = compile_source("globalThis");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_neg_infinity_ok() {
    let module = compile_source("1 / -Infinity");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_class_expression_emits_constructor_value() {
    let module = compile_source("const C = class Foo { method() { return 2; } }; C");
    assert_eq!(module.sub_modules.len(), 2, "expected constructor + method submodules");
    assert!(
        module
            .bytecode
            .iter()
            .filter(|&&i| opcode::opcode(i) == OpCode::SET_PROP)
            .count()
            >= 3,
        "class expression should wire method/constructor/prototype properties"
    );
}

#[test]
fn compile_class_expression_default_constructor() {
    let module = compile_source("class A {}; (class B {})");
    assert!(
        module.sub_modules.iter().any(|m| m.is_class_constructor),
        "expected at least one class constructor submodule"
    );
}
