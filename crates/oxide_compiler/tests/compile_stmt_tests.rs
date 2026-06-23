use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;

fn compile_source(source: &str) -> CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

#[test]
fn compile_var_declaration() {
    let module = compile_source("var x = 42;");
    assert!(!module.bytecode.is_empty());
    assert_eq!(module.constants[0], Constant::Int(42));
}

#[test]
fn compile_return_nothing() {
    let module = compile_source("function f() { return; }");
    let last = opcode::opcode(*module.bytecode.last().unwrap());
    assert_eq!(last, OpCode::HALT);
}

#[test]
fn compile_return_value() {
    let module = compile_source("function f() { return 42; }");
    assert!(!module.bytecode.is_empty(), "function declaration should produce bytecode");
}

#[test]
fn compile_if_else_emits_jmp_if_false() {
    let module = compile_source("if (true) { 1 } else { 2 }");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "if/else should contain JMP_IF_FALSE"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP),
        "if/else should contain JMP"
    );
}

#[test]
fn compile_while_emits_jump_back() {
    let module = compile_source("while (true) { 1 }");
    let jmp_ops: Vec<_> = module.bytecode.iter().filter(|&&i| opcode::opcode(i) == OpCode::JMP).collect();
    assert!(!jmp_ops.is_empty(), "while should contain JMP (backward)");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "while should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_for_emits_jumps() {
    let module = compile_source("for (i=0; i<3; i=i+1) { 1 }");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "for should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_break_in_loop() {
    let module = compile_source("while (true) { break; }");
    assert!(!module.bytecode.is_empty(), "break should compile without error");
}

#[test]
fn compile_continue_in_loop() {
    let module = compile_source("while (true) { continue; }");
    assert!(!module.bytecode.is_empty(), "continue should compile without error");
}

#[test]
fn compile_break_outside_loop_errors() {
    let result = std::panic::catch_unwind(|| {
        compile_source("break;");
    });
    assert!(result.is_err(), "break outside loop should error");
}

#[test]
fn compile_continue_outside_loop_errors() {
    let result = std::panic::catch_unwind(|| {
        compile_source("continue;");
    });
    assert!(result.is_err(), "continue outside loop should error");
}

#[test]
fn compile_nested_if() {
    let module = compile_source("var a=1,b=0; if (a) { if (b) { 1 } else { 2 } }");
    let jmp_if_false_count = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE)
        .count();
    assert!(jmp_if_false_count >= 2, "nested if should have 2+ JMP_IF_FALSE");
}

#[test]
fn compile_empty_while_body() {
    let module = compile_source("while (false) {}");
    assert!(!module.bytecode.is_empty(), "empty while should compile");
}

#[test]
fn regression_for_var_init() {
    let module = compile_source("for (var i = 0; i < 3; i = i + 1) {}");
    assert!(!module.bytecode.is_empty(), "for(var) init should compile");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "for(var) init should contain JMP_IF_FALSE"
    );
}

#[test]
fn regression_var_decl_counter_no_init() {
    let result = std::panic::catch_unwind(|| {
        compile_source("for (var i; i < 3; i = i + 1) {}");
    });
    assert!(
        result.is_err(),
        "for(var) without initializer should fail - accessing TDZ variable in test expression"
    );
}

#[test]
fn regression_redundant_jmp_if_without_else() {
    let module = compile_source("if (true) { 1 }");
    assert!(!module.bytecode.is_empty(), "if without else should compile");
    let last = opcode::opcode(*module.bytecode.last().unwrap());
    assert_eq!(last, OpCode::HALT, "final opcode should be HALT, not dangling JMP");
}

#[test]
fn compile_do_while_emits_jmp_if_true() {
    let module = compile_source("do { 1 } while (true)");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_TRUE),
        "do-while should contain JMP_IF_TRUE"
    );
}

#[test]
fn compile_do_while_emits_body_before_test() {
    let module = compile_source("var x = true; do { 1 } while (x)");
    let has_opcodes_before_jmp = module
        .bytecode
        .iter()
        .take_while(|&&i| opcode::opcode(i) != OpCode::JMP_IF_TRUE)
        .count()
        > 1;
    assert!(has_opcodes_before_jmp, "body should emit before test");
}

#[test]
fn compile_do_while_break() {
    let module = compile_source("do { break; } while (true)");
    assert!(!module.bytecode.is_empty(), "do-while with break should compile");
}

#[test]
fn compile_do_while_continue() {
    let module = compile_source("var x = 0; do { continue; } while (x < 5)");
    assert!(!module.bytecode.is_empty(), "do-while with continue should compile");
}

#[test]
fn compile_for_in_emits_opcodes() {
    let module = compile_source("var obj={a:1}; for (k in obj) {}");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::FOR_IN_INIT),
        "for-in should emit FOR_IN_INIT"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::FOR_IN_NEXT),
        "for-in should emit FOR_IN_NEXT"
    );
}

#[test]
fn compile_for_in_var_decl() {
    let module = compile_source("var obj={a:1}; for (var k in obj) { k }");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_for_in_let_decl() {
    let module = compile_source("var obj={a:1}; for (let k in obj) { k }");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_switch_emits_jmp_if_true() {
    let module = compile_source("var x=0; switch(x){case 1:1;case 2:2;}");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_TRUE),
        "switch should emit JMP_IF_TRUE"
    );
}

#[test]
fn compile_switch_default() {
    let module = compile_source("var x=0; switch(x){default:0;}");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_switch_break() {
    let module = compile_source("var x=0; switch(x){case 1:break;}");
    assert!(!module.bytecode.is_empty());
}

#[test]
fn compile_continue_in_switch_errors() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var x=0; switch(x){case 1:continue;}");
    });
    assert!(result.is_err(), "continue inside switch should error");
}

#[test]
fn compile_large_adjacent_switches_do_not_collide_labels() {
    let mut source = String::from("var r=0;switch(256){");
    for i in 0..=256 {
        source.push_str(&format!("case {i}:"));
        if i == 256 {
            source.push_str("r=1;break;");
        }
    }
    source.push_str("}switch(0){case 0:r=r+1;break;}r");
    let module = compile_source(&source);
    assert!(
        module
            .bytecode
            .iter()
            .filter(|&&i| opcode::opcode(i) == OpCode::JMP_IF_TRUE)
            .count()
            >= 258,
        "expected both large switches to compile with distinct case labels"
    );
}

#[test]
fn compile_class_declaration_emits_constructor_and_prototype_setup() {
    let module = compile_source("class A { m() { return 1; } }");
    assert_eq!(module.sub_modules.len(), 2, "expected constructor + method submodules");
    assert!(
        module
            .bytecode
            .iter()
            .filter(|&&i| opcode::opcode(i) == OpCode::NEW_OBJECT)
            .count()
            >= 1,
        "class should allocate a prototype object"
    );
    assert!(
        module
            .bytecode
            .iter()
            .filter(|&&i| opcode::opcode(i) == OpCode::SET_PROP)
            .count()
            >= 3,
        "class should assign method, constructor, and prototype properties"
    );
}

#[test]
fn compile_class_declaration_default_constructor_creates_submodule() {
    let module = compile_source("class A {}");
    assert_eq!(module.sub_modules.len(), 1, "expected synthesized default constructor");
    assert!(
        module.sub_modules[0].is_class_constructor,
        "default constructor submodule should be marked as class constructor"
    );
}

#[test]
fn compile_class_instance_getter_emits_define_accessor() {
    let module = compile_source("class A { get x() { return 1; } }");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEFINE_ACCESSOR),
        "class getter should emit DEFINE_ACCESSOR"
    );
}

#[test]
fn compile_class_static_setter_emits_define_accessor() {
    let module = compile_source("class A { static set x(v) { this.y = v; } }");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEFINE_ACCESSOR),
        "static class setter should emit DEFINE_ACCESSOR"
    );
}
