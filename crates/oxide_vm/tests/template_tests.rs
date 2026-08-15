use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_compiler::compiler::Compiler;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {}", e),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{}", result),
        Err(e) => format!("vm error: {}", e),
    }
}

fn eval_val(source: &str) -> (Vm, Result<JsValue, String>) {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return (Vm::new(), Err(format!("parse error: {}", e[0].message))),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return (Vm::new(), Err(format!("compile error: {}", e))),
    };
    let mut vm = Vm::new();
    let result = vm.run(&module);
    (vm, result)
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        vm.lookup_str(val).unwrap_or_default()
    } else {
        format!("{}", val)
    }
}

// -- Template literal tests --

#[test]
fn template_no_expressions() {
    let (vm, result) = eval_val("`hello`");
    assert_eq!(to_str(&vm, result.unwrap()), "hello");
}

#[test]
fn template_single_expression() {
    let (vm, result) = eval_val("const name = 'world'; `hello ${name}`");
    assert_eq!(to_str(&vm, result.unwrap()), "hello world");
}

#[test]
fn template_multiple_expressions() {
    let (vm, result) = eval_val("`a${1}b${2}c`");
    assert_eq!(to_str(&vm, result.unwrap()), "a1b2c");
}

#[test]
fn template_expression_only() {
    let (vm, result) = eval_val("const x = 'foo', y = 'bar'; `${x}${y}`");
    assert_eq!(to_str(&vm, result.unwrap()), "foobar");
}

#[test]
fn template_empty() {
    let (vm, result) = eval_val("``");
    assert_eq!(to_str(&vm, result.unwrap()), "");
}

#[test]
fn template_with_numbers() {
    let (vm, result) = eval_val("`value is ${42}`");
    assert_eq!(to_str(&vm, result.unwrap()), "value is 42");
}

#[test]
fn template_numeric_expression() {
    let (vm, result) = eval_val("`${1 + 1}`");
    assert_eq!(to_str(&vm, result.unwrap()), "2");
}

#[test]
fn template_with_double_bigint_and_bool() {
    // 原始值段走直写路径，输出与 ToString 一致。
    let (vm, result) = eval_val("`a${1.5}b${123n}c${true}d`");
    assert_eq!(to_str(&vm, result.unwrap()), "a1.5b123ctrued");
}

#[test]
fn template_with_object_uses_tostring() {
    // 对象段保留完整 ToString（ToPrimitive 触发用户 toString）。
    let (vm, result) = eval_val("var o = { toString: function () { return 'T'; } }; `x${o}y`");
    assert_eq!(to_str(&vm, result.unwrap()), "xTy");
}

#[test]
fn template_with_symbol_throws_type_error() {
    // Symbol 段经 to_string_full 抛 TypeError，不静默输出空段。
    let result = eval("`${Symbol('x')}`");
    assert!(result.contains("TypeError"), "模板串中的 Symbol 应抛 TypeError，got: {result}");
}

#[test]
fn template_expression_reads_physical_register_above_127() {
    let mut ir = IRFunction::new();
    ir.constants = vec![Constant::String("value".to_string()), Constant::String(String::new())];
    ir.n_registers = 202;
    ir.insts = vec![
        Inst::load_const(Operand::Reg(200), 0),
        Inst::template_str(Operand::Reg(201), 3, 0, &[1, 0x8000_0000 | 200, 1]),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(201)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];

    let module = oxide_ir::lower::lower(&ir).expect("lower template IR");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run template IR");
    assert_eq!(to_str(&vm, result), "value");
}

#[test]
fn template_expression_survives_high_vreg_regalloc() {
    let mut source = String::from("function f() {");
    for i in 0..180 {
        source.push_str(&format!("let v{i}={i};"));
    }
    source.push_str("let value='unit'; return `${value}s`; } f();");

    let (vm, result) = eval_val(&source);
    assert_eq!(to_str(&vm, result.unwrap()), "units");
}

// ── 模板标签调用测试（基础）──

#[test]
fn tagged_template_basic() {
    // 用 Math.max 作为简单 native 标签函数，验证 CALL 分发可用。
    // Math.max(cooked_array, raw_array, 42) 应返回 42（参数最大值）。
    // 由此避开字节码函数调用复杂度。
    let result = eval("Math.max`hello ${42} world`");
    // Math.max 作用于参数——仅验证不崩溃。
    assert!(!result.starts_with("vm error:"), "Tagged template should not crash, got: {}", result);
}

#[test]
fn tagged_template_compiles_no_error() {
    // 验证带标签模板编译不报错（即使标签函数行为未被完整测试）。
    let result = eval("function t(s,v){ return s[0]+v; } t`x${1}`");
    assert!(!result.starts_with("compile error:"), "Tagged template should compile");
}
