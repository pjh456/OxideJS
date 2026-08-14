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

#[allow(dead_code)]
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
fn test_constant_pool_overflow_range_error() {
    let expr = (0..70_000).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let source = format!("var a=[{expr}]; a");
    let err = compile_source_err(&source);
    assert!(err.contains("too many constants"), "unexpected error: {err}");
}

#[test]
fn compile_logical_assignment_vars() {
    compile_source("let x = 0; x ||= 1;");
    compile_source("let x = 1; x &&= 2;");
    compile_source("let x = null; x ??= 3;");
}

#[test]
fn compile_logical_assignment_members() {
    compile_source("let obj = { x: 0 }; obj.x ||= 1;");
    compile_source("let obj = { x: 1 }; obj.x &&= 2;");
    compile_source("let obj = { x: null }; obj.x ??= 3;");
}

#[test]
fn compile_constants() {
    let module = compile_source("42");

    assert!(!module.constants.is_empty());
    assert_eq!(module.constants[0], Constant::Int(42));
}

#[test]
fn compile_dedups_identical_string_constants() {
    let module = compile_source("var a = 'hello'; var b = 'hello';");
    let count = module
        .constants
        .iter()
        .filter(|c| matches!(c, Constant::String(s) if s == "hello"))
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

    // Compiler 默认 DCE 开启：死 `1; 2` 链的 LOAD_CONST 被删除，
    // 仅剩顶层值 `3` 的 LOAD_CONST（HALT 经 reg0 返回）。精确计数断言放宽为存在性断言。
    let has_load = module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::LOAD_CONST);
    assert!(has_load, "expected at least one LOAD_CONST for the final value");
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
fn compile_object_literal_pure_data_keys_emit_batch_construction() {
    let module = compile_source("var o = { a: 1, b: 2 };");
    let new_obj: Vec<u32> = module
        .bytecode
        .iter()
        .copied()
        .filter(|&i| opcode::opcode(i) == OpCode::NEW_OBJECT)
        .collect();
    assert_eq!(new_obj.len(), 1, "exactly one NEW_OBJECT");
    assert_eq!(opcode::a(new_obj[0]), 2, "NEW_OBJECT a 槽编码属性数");
    let batch_writes = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::SET_PROP_BATCH)
        .count();
    assert_eq!(batch_writes, 2, "批量前缀逐属性发 SET_PROP_BATCH");
    let slow_writes = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::SET_PROP)
        .count();
    assert_eq!(slow_writes, 0, "纯数据键字面量不应再发慢路径 SET_PROP");
}

#[test]
fn compile_object_literal_computed_key_terminates_batch_prefix() {
    let module = compile_source("var o = { a: 1, [k]: 2 };");
    let new_obj: Vec<u32> = module
        .bytecode
        .iter()
        .copied()
        .filter(|&i| opcode::opcode(i) == OpCode::NEW_OBJECT)
        .collect();
    assert_eq!(opcode::a(new_obj[0]), 1, "computed 键终止批段，前缀仅 a");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::SET_PROP_BATCH),
        "前缀属性仍走批量槽写"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::SET_PROP_DYNAMIC),
        "computed 键后续走动态路径"
    );
}

#[test]
fn compile_object_literal_duplicate_key_falls_back_whole_literal() {
    let module = compile_source("var o = { a: 1, a: 2 };");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::SET_PROP),
        "重复键整字面量回退慢路径"
    );
    assert!(module.bytecode.iter().all(|&i| opcode::opcode(i) != OpCode::SET_PROP_BATCH), "重复键不批");
}

#[test]
fn compile_object_literal_proto_key_falls_back() {
    let module = compile_source("var o = { a: 1, __proto__: null };");
    let new_obj: Vec<u32> = module
        .bytecode
        .iter()
        .copied()
        .filter(|&i| opcode::opcode(i) == OpCode::NEW_OBJECT)
        .collect();
    assert_eq!(opcode::a(new_obj[0]), 1, "__proto__ 终止批段，前缀仅 a");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::SET_PROP),
        "__proto__ 键走慢路径拦截"
    );
}

#[test]
fn compile_object_literal_accessor_not_batched() {
    let module = compile_source("var o = { a: 1, get b() { return 2; } };");
    let new_obj: Vec<u32> = module
        .bytecode
        .iter()
        .copied()
        .filter(|&i| opcode::opcode(i) == OpCode::NEW_OBJECT)
        .collect();
    assert_eq!(opcode::a(new_obj[0]), 1, "accessor 终止批段");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DEFINE_ACCESSOR),
        "accessor 后续走访问器定义路径"
    );
}

#[test]
fn compile_nested_object_literals_both_batch() {
    let module = compile_source("var o = { a: { b: 1 } };");
    let batch_objects = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::NEW_OBJECT && opcode::a(i) > 0)
        .count();
    assert_eq!(batch_objects, 2, "内外层字面量都走批量构造");
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
    let module = compile_source("var a = null; var b = 1; a ?? b");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&instr| opcode::opcode(instr) == OpCode::NULLISH || opcode::opcode(instr) == OpCode::JMP_IF_NULLISH),
        "a ?? b should emit nullish bytecode"
    );
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
fn nullish_opcodes_round_trip() {
    for op in [OpCode::NULLISH, OpCode::JMP_IF_NULLISH] {
        assert_eq!(OpCode::try_from(op as u8), Ok(op));
        assert_eq!(format!("{op}"), format!("{op:?}"));
    }
}

#[test]
fn compile_nullish_fast_path_opcode() {
    let module = compile_source("null ?? 5");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NULLISH),
        "side-effect-free ?? should emit NULLISH"
    );
}

#[test]
fn compile_nullish_short_circuit_opcode() {
    let module = compile_source("var x = 0; x ?? (x = 5)");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::JMP_IF_NULLISH),
        "side-effecting ?? should emit JMP_IF_NULLISH"
    );
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
            .filter(|&&i| opcode::opcode(i) == OpCode::DEFINE_PROP_ATTRS)
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

#[test]
fn compile_large_pure_expression_reuses_temp_registers() {
    let expr = std::iter::repeat("1").take(40).collect::<Vec<_>>().join(" + ");
    let module = compile_source(&expr);
    assert!(
        module.n_registers < 16,
        "pure expression should reuse temp registers, got {}",
        module.n_registers
    );
}

#[test]
fn compile_builtin_globals_are_registered_lazily() {
    // 数组字面量内全部 4 个引用均为活代码（结果即脚本值），DCE 不可删；
    // 未被引用的其它内置全局不得占用槽位。
    let module = compile_source("[Object, Array, Math, JSON]");
    assert_eq!(module.builtin_reg_map.len(), 4, "only referenced builtins should allocate registers");
}

#[test]
fn compile_comparison_complement_ops() {
    // 结果被使用（赋值）→ 比较指令不被 DCE 删除（精确 DCE 会删结果丢弃的死比较）
    let module = compile_source("let a = 1, b = 2; var r = a > b; var s = a <= b; var t = a >= b; var u = a != b;");
    assert!(module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::GT), "a > b should emit GT");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::LTE),
        "a <= b should emit LTE"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::GTE),
        "a >= b should emit GTE"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NEQ),
        "a != b should emit NEQ"
    );
}

#[test]
fn compile_template_literal_emits_template_str() {
    let module = compile_source("let name = 'x'; `hello ${name}`;");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::TEMPLATE_STR),
        "template literal should emit TEMPLATE_STR"
    );
}

#[test]
fn compile_call_spread_emits_spread_opcodes() {
    // 普通调用 / new 表达式含 spread 实参 → 发 spread 变体 opcode
    let module = compile_source("function f(){}; f(...[1,2]); new f(...[3]);");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::CALL_SPREAD),
        "f(...args) should emit CALL_SPREAD"
    );
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::NEW_EXPRESSION_SPREAD),
        "new f(...args) should emit NEW_EXPRESSION_SPREAD"
    );
    // 无 spread 的调用不发 spread 变体
    let plain = compile_source("function f(a){}; f(1);");
    assert!(
        !plain.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::CALL_SPREAD),
        "plain call should not emit CALL_SPREAD"
    );
}

#[test]
fn compile_dynamic_member_ops() {
    let module = compile_source("let obj = {}, k = 'x', arr = [1]; obj[k]; obj[k] = 1; arr[0];");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NEW_ARRAY),
        "array literal should emit NEW_ARRAY"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::GET_PROP_DYNAMIC),
        "obj[k] should emit GET_PROP_DYNAMIC"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::SET_PROP_DYNAMIC),
        "obj[k] = should emit SET_PROP_DYNAMIC"
    );
}

/// 收集字节码指令序列，跳过 IC 扩展字（3 字）与其它扩展字，逐指令定位。
fn scan_opcodes(module: &CompiledModule) -> Vec<OpCode> {
    let mut ops = Vec::new();
    let mut i = 0;
    while i < module.bytecode.len() {
        let op = opcode::opcode(module.bytecode[i]);
        ops.push(op);
        i += 1 + if op.has_ic_ext_words() {
            3
        } else {
            match op {
                OpCode::SPILL
                | OpCode::UNSPILL
                | OpCode::CALL
                | OpCode::CALL_NATIVE
                | OpCode::NEW_EXPRESSION
                | OpCode::SUPER_CALL
                | OpCode::DEFINE_ACCESSOR
                | OpCode::DEFINE_ACCESSOR_DYNAMIC
                | OpCode::DEFINE_PROP_ATTRS
                | OpCode::DELETE_PROP_STATIC
                | OpCode::REST_OBJECT
                | OpCode::INIT_PRIVATE => 1,
                OpCode::DEFINE_ACCESSOR_ATTRS
                | OpCode::GET_PRIVATE
                | OpCode::SET_PRIVATE
                | OpCode::PRIVATE_BRAND_IN => 2,
                _ => 0,
            }
        };
    }
    ops
}

#[test]
fn compile_computed_member_const_string_key_folds_to_ic() {
    let module = compile_source("let obj = { a: 1 }; obj[\"a\"]; obj[\"a\"] = 2;");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::IC_GET_PROP), "obj[\"a\"] 应折叠为 IC_GET_PROP");
    assert!(ops.contains(&OpCode::IC_SET_PROP), "obj[\"a\"] = 应折叠为 IC_SET_PROP");
    assert!(!ops.contains(&OpCode::GET_PROP_DYNAMIC), "常量字符串键读不应发 GET_PROP_DYNAMIC");
    assert!(!ops.contains(&OpCode::SET_PROP_DYNAMIC), "常量字符串键写不应发 SET_PROP_DYNAMIC");
}

#[test]
fn compile_computed_member_template_key_folds_to_ic() {
    let module = compile_source("let obj = { a: 1 }; obj[`a`];");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::IC_GET_PROP), "无插值模板键应折叠为 IC_GET_PROP");
    assert!(!ops.contains(&OpCode::GET_PROP_DYNAMIC), "无插值模板键不应发 GET_PROP_DYNAMIC");
    // 含插值模板保持 DYNAMIC（运行期键非常量）
    let interp = compile_source("let obj = {}, k = 'a'; obj[`${k}`];");
    assert!(
        scan_opcodes(&interp).contains(&OpCode::GET_PROP_DYNAMIC),
        "含插值模板键应保持 GET_PROP_DYNAMIC"
    );
}

#[test]
fn compile_computed_member_numeric_and_index_keys_stay_dynamic() {
    // 数字键（obj[0] / arr[\"0\"]）不折叠：数组元素区 IC 永不命中。
    let module = compile_source("let obj = {}, arr = [1]; obj[0]; arr[\"0\"];");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::GET_PROP_DYNAMIC), "数字/数组下标键应保持 GET_PROP_DYNAMIC");
    assert!(!ops.contains(&OpCode::IC_GET_PROP), "数字/数组下标键不应折叠为 IC_GET_PROP");
}

#[test]
fn compile_computed_member_compound_and_update_fold_to_ic() {
    let module = compile_source("let obj = { a: 1 }; obj[\"a\"] += 1; obj[\"b\"]++;");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::COMPOUND_MEMBER_ADD), "obj[\"a\"] += 应折叠为 COMPOUND_MEMBER_ADD");
    assert!(ops.contains(&OpCode::MEMBER_INC), "obj[\"b\"]++ 应折叠为 MEMBER_INC");
    assert!(!ops.contains(&OpCode::GET_PROP_DYNAMIC), "常量键复合赋值不应发 GET_PROP_DYNAMIC");
}

#[test]
fn compile_computed_member_call_receiver_folds_to_ic() {
    let module = compile_source("let obj = { m() { return 1 } }; obj[\"m\"]();");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::IC_GET_PROP), "obj[\"m\"]() 接收者应折叠为 IC_GET_PROP");
    assert!(!ops.contains(&OpCode::GET_PROP_DYNAMIC), "常量键方法调用不应发 GET_PROP_DYNAMIC");
}

#[test]
fn compile_computed_member_chain_folds_to_ic() {
    let module = compile_source("let obj = { a: 1 }; obj?.[\"a\"];");
    let ops = scan_opcodes(&module);
    assert!(ops.contains(&OpCode::IC_GET_PROP), "可选链 obj?.[\"a\"] 应折叠为 IC_GET_PROP");
    assert!(!ops.contains(&OpCode::GET_PROP_DYNAMIC), "常量键可选链不应发 GET_PROP_DYNAMIC");
}

#[test]
fn compile_delete_new_instanceof_in() {
    let module =
        compile_source("let obj = { x: 1 }, B = function(){}; delete obj.x; new B(); obj instanceof B; 'x' in obj;");
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::DELETE_PROP_STATIC),
        "delete obj.x should emit DELETE_PROP_STATIC"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::NEW_EXPRESSION),
        "new B() should emit NEW_EXPRESSION"
    );
    assert!(
        module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::INSTANCEOF),
        "instanceof should emit INSTANCEOF"
    );
    assert!(module.bytecode.iter().any(|&i| opcode::opcode(i) == OpCode::IN), "in should emit IN");
}
