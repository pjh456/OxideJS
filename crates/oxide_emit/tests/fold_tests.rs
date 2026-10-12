//! 双常量折叠的 IR 级结构断言：parse → `emit_program` 为止，钉死折叠命中、
//! 语义守卫不折与发射确定性。
//!
//! 命中矩阵断言目标操作码计数为 0 且常量池含折值；守卫矩阵断言目标操作码
//! 计数为 1（落回运行时路径）；确定性断言同源串连续编译八字节码指纹全等。

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_emit::Emitter;
use oxide_ir::inst::Inst;
use oxide_ir::IRFunction;

/// 夹具：parse + 脚本口径 emit，失败 panic 带源串。
fn emit_ir(src: &str) -> IRFunction {
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, src).unwrap_or_else(|errs| panic!("parse 失败 {src:?}: {errs:?}"));
    Emitter::new()
        .emit_program(&program, false, false)
        .unwrap_or_else(|e| panic!("emit 失败 {src:?}: {e}"))
}

fn count_op(insts: &[Inst], op: OpCode) -> usize {
    insts.iter().filter(|i| i.op == op).count()
}

/// 常量池含指定常量（按 `PartialEq` 精确匹配，`Int(3)` 与 `Number(3.0)` 是不同键）。
fn pool_contains(ir: &IRFunction, c: &Constant) -> bool {
    ir.constants.contains(c)
}

/// 命中矩阵：每行一条源串，断言目标操作码计数为 0 且常量池含折值。
/// 折值编码逐运算符镜像运行时产物（ADD 保 Int、SUB/MUL/DIV/MOD/EXP 恒 f64、
/// 位运算 Int、USHR 越 i32::MAX 升 Number、比较/等值 Boolean）。
#[test]
fn fold_hit_matrix() {
    // (源串, 目标操作码, 期望折值)
    let cases: &[(&str, OpCode, Constant)] = &[
        ("1 + 2", OpCode::ADD, Constant::Int(3)),
        ("1 - 2", OpCode::SUB, Constant::Number(-1.0)),
        ("2 * 3", OpCode::MUL, Constant::Number(6.0)),
        ("1 / 2", OpCode::DIV, Constant::Number(0.5)),
        ("7 % 3", OpCode::MOD, Constant::Number(1.0)),
        ("2 ** 10", OpCode::EXP, Constant::Number(1024.0)),
        ("5 & 3", OpCode::BIT_AND, Constant::Int(1)),
        ("5 | 3", OpCode::BIT_OR, Constant::Int(7)),
        ("5 ^ 3", OpCode::BIT_XOR, Constant::Int(6)),
        // 移位量模 32：34 模 32 为 2。
        ("1 << 34", OpCode::SHL, Constant::Int(4)),
        ("-8 >> 1", OpCode::SHR, Constant::Int(-4)),
        // USHR 越 i32::MAX 升 Number。
        ("-1 >>> 0", OpCode::USHR, Constant::Number(4294967295.0)),
        ("1 < 2", OpCode::LT, Constant::Boolean(true)),
        ("2 > 1", OpCode::GT, Constant::Boolean(true)),
        ("1 <= 1", OpCode::LTE, Constant::Boolean(true)),
        ("1 >= 2", OpCode::GTE, Constant::Boolean(false)),
        // 布尔经 to_number。
        ("true < 2", OpCode::LT, Constant::Boolean(true)),
        ("null < 1", OpCode::LT, Constant::Boolean(true)),
        // int 与 double 混合表示按数值。
        ("1 === 1.0", OpCode::STRICT_EQ, Constant::Boolean(true)),
        ("1 == 1.0", OpCode::EQ, Constant::Boolean(true)),
        // 宽松相等 Boolean 重入。
        ("true == 1", OpCode::EQ, Constant::Boolean(true)),
        ("null == null", OpCode::EQ, Constant::Boolean(true)),
        ("1 !== 2", OpCode::STRICT_NEQ, Constant::Boolean(true)),
        // 括号剥壳。
        ("(1) + (2)", OpCode::ADD, Constant::Int(3)),
    ];
    for (src, op, folded) in cases {
        let ir = emit_ir(src);
        assert_eq!(count_op(&ir.insts, *op), 0, "{src:?} 应折叠，目标操作码 {op:?} 计数应为 0");
        assert!(pool_contains(&ir, folded), "{src:?} 常量池应含折值 {folded:?}");
    }
}

/// 守卫不折矩阵：断言目标操作码计数为 1（落回运行时路径）。
/// 除零守卫须同时覆盖 Int(0) 与 Number(±0.0)；负指数守卫须同时覆盖 Int/Number
/// 负值；BigInt 字面量、标识符均不折；字符串域守卫（全数字链无字符串字面量）
/// 落回 CONCAT_N。
#[test]
fn fold_guard_matrix() {
    // (源串, 目标操作码)
    let cases: &[(&str, OpCode)] = &[
        // DIV/MOD 除零（Int 0 与 Number ±0.0）。
        ("1 / 0", OpCode::DIV),
        ("1 / -0.0", OpCode::DIV),
        ("1 % 0", OpCode::MOD),
        // EXP 负指数。
        ("2 ** -1", OpCode::EXP),
        // BigInt 字面量（BigInt 混合、BigInt-BigInt 独立指令族）。
        ("1n + 2n", OpCode::ADD),
        ("1 + 1n", OpCode::ADD),
        ("1n < 2n", OpCode::LT),
        // 字符串域守卫：全数字链无字符串字面量，不落字符串域，落回 CONCAT_N。
        ("1 + 2 + 3", OpCode::CONCAT_N),
        // 标识符不折。
        ("1 + x", OpCode::ADD),
    ];
    for (src, op) in cases {
        let ir = emit_ir(src);
        assert_eq!(count_op(&ir.insts, *op), 1, "{src:?} 守卫命中，目标操作码 {op:?} 应保留 1 条");
    }
}

/// 一元/逻辑/条件命中矩阵：每行一条源串，断言目标操作码计数为 0 且常量池含折值。
/// 折值编码逐运算符镜像运行时产物（`NEG`/`UNARY_PLUS` 恒 `Number`、`BIT_NOT` 恒
/// `Int`、`-`/`~` 对 BigInt 折 `BigInt`、`NOT` 恒 `Boolean`、`TYPEOF` 恒 `String`、
/// `VOID` 恒 `Undefined`、逻辑与条件恒操作数原样拷贝）。
#[test]
fn fold_unary_logical_conditional_hit_matrix() {
    // (源串, 目标操作码, 期望折值)
    let cases: &[(&str, OpCode, Constant)] = &[
        // 一元：NOT。
        ("!0", OpCode::NOT, Constant::Boolean(true)),
        ("!1", OpCode::NOT, Constant::Boolean(false)),
        ("!0n", OpCode::NOT, Constant::Boolean(true)),
        ("!1n", OpCode::NOT, Constant::Boolean(false)),
        // 一元：NEG 恒 Number（不保 Int），BigInt 折 BigInt。
        ("-1", OpCode::NEG, Constant::Number(-1.0)),
        ("-1.5", OpCode::NEG, Constant::Number(-1.5)),
        // 双层负号经剥壳。
        ("-(-1)", OpCode::NEG, Constant::Number(1.0)),
        ("-10n", OpCode::NEG, Constant::BigInt(num_bigint::BigInt::from(-10))),
        // 一元：UNARY_PLUS 恒 Number。
        ("+1", OpCode::UNARY_PLUS, Constant::Number(1.0)),
        ("+true", OpCode::UNARY_PLUS, Constant::Number(1.0)),
        ("+null", OpCode::UNARY_PLUS, Constant::Number(0.0)),
        // 负零守卫：奇数层负号作用于 0 字面量保 Number(-0.0)。
        ("+(-0.0)", OpCode::UNARY_PLUS, Constant::Number(-0.0)),
        // 一元：BIT_NOT 恒 Int，BigInt 折 BigInt。
        ("~1", OpCode::BIT_NOT, Constant::Int(-2)),
        ("~true", OpCode::BIT_NOT, Constant::Int(-2)),
        ("~10n", OpCode::BIT_NOT, Constant::BigInt(num_bigint::BigInt::from(-11))),
        // 一元：TYPEOF 恒 String（BigInt 归 "bigint"，独立于 "number"）。
        ("typeof 1", OpCode::TYPEOF, Constant::String("number".into())),
        ("typeof true", OpCode::TYPEOF, Constant::String("boolean".into())),
        ("typeof null", OpCode::TYPEOF, Constant::String("object".into())),
        ("typeof 1n", OpCode::TYPEOF, Constant::String("bigint".into())),
        // 一元：VOID 恒 Undefined。
        ("void 1", OpCode::VOID, Constant::Undefined),
        ("void 0n", OpCode::VOID, Constant::Undefined),
        // 逻辑：恒操作数原样拷贝。
        ("1 && 2", OpCode::AND, Constant::Int(2)),
        ("0 && 2", OpCode::AND, Constant::Int(0)),
        ("true || 2", OpCode::OR, Constant::Boolean(true)),
        ("0 || 2", OpCode::OR, Constant::Int(2)),
        ("1n && 2n", OpCode::AND, Constant::BigInt(num_bigint::BigInt::from(2))),
        ("0n && 2n", OpCode::AND, Constant::BigInt(num_bigint::BigInt::from(0))),
        ("null ?? 1", OpCode::NULLISH, Constant::Int(1)),
        ("1 ?? 2", OpCode::NULLISH, Constant::Int(1)),
        ("1n ?? 2n", OpCode::NULLISH, Constant::BigInt(num_bigint::BigInt::from(1))),
        // 条件：test 布尔值选定分支。
        ("1 ? 2 : 3", OpCode::JMP_IF_FALSE, Constant::Int(2)),
        ("0 ? 2 : 3", OpCode::JMP_IF_FALSE, Constant::Int(3)),
        ("true ? 1n : 2n", OpCode::JMP_IF_FALSE, Constant::BigInt(num_bigint::BigInt::from(1))),
    ];
    for (src, op, folded) in cases {
        let ir = emit_ir(src);
        assert_eq!(count_op(&ir.insts, *op), 0, "{src:?} 应折叠，目标操作码 {op:?} 计数应为 0");
        assert!(pool_contains(&ir, folded), "{src:?} 常量池应含折值 {folded:?}");
    }
}

/// 一元/逻辑/条件守卫矩阵：非字面量操作数落回运行时路径，每条带显式期望计数
/// （落回路径操作码因运算符而异，不复用二元守卫的「计数为 1」单操作码断言）。
#[test]
fn fold_unary_logical_conditional_guard_matrix() {
    // (源串, 目标操作码, 期望计数)
    let cases: &[(&str, OpCode, usize)] = &[
        // + 作用 BigInt：运行期 TypeError 唯一来源，不折。
        ("+1n", OpCode::UNARY_PLUS, 1),
        // 剥壳后仍 BigInt。
        ("+(-1n)", OpCode::UNARY_PLUS, 1),
        // typeof 操作数为二元表达式（非字面量），不折。
        ("typeof (1 + 2)", OpCode::TYPEOF, 1),
        // typeof 未声明标识符走 LOAD_GLOBAL_TYPEOF 特判加 TYPEOF，不折。
        ("typeof x", OpCode::TYPEOF, 1),
        // 逻辑短路路径（非字面量操作数）。
        ("x && 1", OpCode::AND, 0),
        ("1 && x", OpCode::AND, 0),
        ("x || 1", OpCode::OR, 0),
        ("x ?? 1", OpCode::NULLISH, 0),
        // 条件：consequent / test 非字面量。
        ("1 ? x : 2", OpCode::JMP_IF_FALSE, 1),
        ("x ? 1 : 2", OpCode::JMP_IF_FALSE, 1),
    ];
    for (src, op, expected) in cases {
        let ir = emit_ir(src);
        assert_eq!(
            count_op(&ir.insts, *op),
            *expected,
            "{src:?} 守卫命中，目标操作码 {op:?} 计数应为 {expected}"
        );
    }
    // typeof 未声明标识符走 LOAD_GLOBAL_TYPEOF 特判路径。
    let ir = emit_ir("typeof x");
    assert_eq!(count_op(&ir.insts, OpCode::LOAD_GLOBAL_TYPEOF), 1, "typeof x 应发 LOAD_GLOBAL_TYPEOF");
}

/// 字节码指纹：`CompiledModule` 未派生 `Debug`，逐字段展开为确定串（子模块递归）。
fn bytecode_fingerprint(m: &oxide_bytecode::module::CompiledModule) -> String {
    let mut out = format!(
        "bc={:?};k={:?};nr={};na={};pb={};bim={:?};uv={:?};cn={};",
        m.bytecode,
        m.constants,
        m.n_registers,
        m.n_args,
        m.param_base,
        m.builtin_reg_map,
        m.upvalue_captures,
        m.cells_needed
    );
    for sub in &m.sub_modules {
        out.push_str(&bytecode_fingerprint(sub));
    }
    out
}

/// 确定性：同一源串连续编译八次，IR 与字节码指纹全等（折叠路径纯函数无迭代序依赖）。
#[test]
fn fold_deterministic() {
    let srcs = [
        "1 + 2 * 3;",
        "var a = -1 >>> 0; var b = 1 < 2;",
        "var c = 5 & 3 | 4;",
        "var a = !1 && 2; var b = 1 ? 2 : 3; var c = typeof 1n; var d = 0n ?? 1n;",
    ];
    for src in srcs {
        let mut fingerprints: Vec<(String, String)> = Vec::new();
        for _ in 0..8 {
            let alloc = oxide_parser::Allocator::default();
            let program = oxide_parser::parse(&alloc, src).unwrap_or_else(|e| panic!("parse 失败 {src:?}: {e:?}"));
            let ir = Emitter::new()
                .emit_program(&program, false, false)
                .unwrap_or_else(|e| panic!("emit 失败 {src:?}: {e}"));
            let bytecode = oxide_ir::lower::lower(&ir).unwrap_or_else(|e| panic!("lower 失败 {src:?}: {e}"));
            fingerprints.push((format!("{ir:?}"), bytecode_fingerprint(&bytecode)));
        }
        let first = &fingerprints[0];
        for (i, fp) in fingerprints.iter().enumerate().skip(1) {
            assert_eq!(fp.0, first.0, "第 {i} 次编译 IR 指纹漂移：{src:?}");
            assert_eq!(fp.1, first.1, "第 {i} 次编译字节码指纹漂移：{src:?}");
        }
    }
}

/// 负零字面量符号保真：折叠后常量池须含 `Number(-0.0)`（符号位置位），
/// 不得走 i32 臂（`-0.0 as i32` 丢符号成 `Int(0)`）。符号位经 `to_bits`
/// 比对——`Number(-0.0)` 与 `Number(0.0)` 的 `PartialEq` 相等，不能用
/// `pool_contains` 判别。
#[test]
fn fold_negative_zero_sign() {
    let cases: &[(&str, OpCode)] = &[
        ("-0 - 0", OpCode::SUB),
        ("-0 * 1", OpCode::MUL),
        ("-0 / 1", OpCode::DIV),
        ("-0 % 1", OpCode::MOD),
        ("(-0) ** 3", OpCode::EXP),
    ];
    let neg_zero_bits = (-0.0f64).to_bits();
    for (src, op) in cases {
        let ir = emit_ir(src);
        assert_eq!(
            count_op(&ir.insts, *op),
            0,
            "{src:?} 应折叠，目标操作码 {op:?} 计数应为 0"
        );
        let has_neg_zero = ir
            .constants
            .iter()
            .any(|c| matches!(c, Constant::Number(d) if d.to_bits() == neg_zero_bits));
        assert!(
            has_neg_zero,
            "{src:?} 常量池应含折值 Number(-0.0)（符号位置位）"
        );
    }
}
