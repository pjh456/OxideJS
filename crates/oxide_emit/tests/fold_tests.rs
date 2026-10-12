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
/// 负值；BigInt 字面量、字符串字面量、标识符均不折。
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
        // 字符串域不折。
        ("\"a\" + \"b\"", OpCode::ADD),
        // 标识符不折。
        ("1 + x", OpCode::ADD),
    ];
    for (src, op) in cases {
        let ir = emit_ir(src);
        assert_eq!(count_op(&ir.insts, *op), 1, "{src:?} 守卫命中，目标操作码 {op:?} 应保留 1 条");
    }
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
    let srcs = ["1 + 2 * 3;", "var a = -1 >>> 0; var b = 1 < 2;", "var c = 5 & 3 | 4;"];
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
