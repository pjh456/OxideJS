//! 脚本顶层形态的 IR 级结构断言：parse → `emit_program` 为止，钉死现状 emit 行为。
//!
//! 断言顺序无关：只表达 存在 / 计数 / 名字集 / 键名集。GDI 序言对组与 var 入口
//! MAKE_CELL 组的指令序逐进程轮换，这两组只断言键名集；断言不依赖寄存器号
//! 与 `n_registers` 绝对值。

use std::collections::HashMap;
use std::collections::HashSet;

use oxide_bytecode::opcode::OpCode;
use oxide_emit::{Constant, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
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

fn tree_count(ir: &IRFunction, op: OpCode) -> usize {
    let mut n = count_op(&ir.insts, op);
    for sub in &ir.nested {
        n += tree_count(sub, op);
    }
    n
}

fn pool_strings(ir: &IRFunction) -> HashSet<String> {
    ir.constants
        .iter()
        .filter_map(|c| if let Constant::String(s) = c { Some(s.clone()) } else { None })
        .collect()
}

/// 寄存器 → 其被 LOAD_CONST 装入的常量（vreg 号单调不复用，映射唯一）。
fn loaded_consts(ir: &IRFunction) -> HashMap<u32, &Constant> {
    let mut m: HashMap<u32, &Constant> = HashMap::new();
    for i in &ir.insts {
        if i.op == OpCode::LOAD_CONST {
            if let (Operand::Reg(r), Operand::Const(c)) = (&i.rd, &i.a) {
                m.insert(*r, &ir.constants[*c as usize]);
            }
        }
    }
    m
}

/// 指定 opcode 的 b 槽键寄存器回溯其 LOAD_CONST 装入的池字符串名集。
/// 用于 GDI 序言对组（`DEFINE_GLOBAL_PROP_IF_ABSENT`）与顶层 A 侧全局写
/// （`DEFINE_GLOBAL_PROP`）的键名集断言（对序逐进程轮换，只按名集断言）。
fn key_names_of(ir: &IRFunction, op: OpCode) -> HashSet<String> {
    let loads = loaded_consts(ir);
    let mut keys = HashSet::new();
    for i in &ir.insts {
        if i.op == op {
            if let Operand::Reg(r) = &i.b {
                if let Some(Constant::String(s)) = loads.get(r) {
                    keys.insert(s.clone());
                }
            }
        }
    }
    keys
}

fn gdi_key_names(ir: &IRFunction) -> HashSet<String> {
    key_names_of(ir, OpCode::DEFINE_GLOBAL_PROP_IF_ABSENT)
}

fn define_global_prop_keys(ir: &IRFunction) -> HashSet<String> {
    key_names_of(ir, OpCode::DEFINE_GLOBAL_PROP)
}

fn class_flags_scan(f: &IRFunction, base: &mut bool, derived: &mut bool, home: &mut bool) {
    if f.is_class_constructor && !f.is_derived_constructor {
        *base = true;
    }
    if f.is_class_constructor && f.is_derived_constructor {
        *derived = true;
    }
    if f.needs_home_object {
        *home = true;
    }
    for n in &f.nested {
        class_flags_scan(n, base, derived, home);
    }
}

/// 面 1：GDI 序言只覆盖顶层 var 名，lexical（let/const）不进序言。
#[test]
fn gdi_prologue_covers_var_not_lexical() {
    let ir = emit_ir("var a = 1; var b = 2; let c = 3;");
    assert_eq!(
        gdi_key_names(&ir),
        ["a", "b"].map(String::from).into_iter().collect(),
        "GDI 序言键名集应恰为顶层 var 名"
    );
}

/// 面 2：顶层函数声明的 GDI 检查、全局绑定与闭包物化形态。
#[test]
fn top_level_function_declaration() {
    let ir = emit_ir("function f(n) { return n + 1; }");
    assert_eq!(count_op(&ir.insts, OpCode::CAN_DECLARE_GLOBAL_FUNC), 1);
    assert_eq!(count_op(&ir.insts, OpCode::DEFINE_GLOBAL_FUNC_BIND), 1);
    assert_eq!(count_op(&ir.insts, OpCode::CREATE_CLOSURE), 1);
    assert_eq!(ir.nested.len(), 1);
    assert!(ir.is_top_level);
    let f = &ir.nested[0];
    assert_eq!(f.function_name.as_deref(), Some("f"));
    assert_eq!(f.function_length, 1);
    assert!(!f.is_top_level);
}

/// 面 3：块级函数入口物化——每块函数全流恰 1 个 CREATE_CLOSURE（声明点不重编），
/// 外层 var 写回存在；三常量名（NaN）被守卫：无 GDI 对、无 A 侧全局写。
#[test]
fn block_fn_entry_materialized_once() {
    let src = "{ function g() { return 2; } var y = g(); }\n\
               label: { function h() { return 3; } }\n\
               for (var i = 0; i < 1; i++) { function f() { return i; } }\n\
               label3: { function NaN() { return 0; } }";
    let ir = emit_ir(src);
    assert_eq!(tree_count(&ir, OpCode::CREATE_CLOSURE), 4, "块函数数 = 闭包物化数");
    assert_eq!(
        gdi_key_names(&ir),
        ["f", "g", "h", "i", "y"].map(String::from).into_iter().collect(),
        "GDI 键名集应含外层 var 与块函数泄漏名，不含三常量名 NaN"
    );
    assert_eq!(
        define_global_prop_keys(&ir),
        ["f", "g", "h", "i"].map(String::from).into_iter().collect(),
        "A 侧全局写键名集不含 NaN（不可写内置守卫）"
    );
    assert!(count_op(&ir.insts, OpCode::STORE_VAR) >= 1, "外层 var 写回 STORE_VAR 应存在");
}

/// 面 4：三级闭包链的 cell 分配与 upvalue 捕获（cell 下标按名排序分配）。
#[test]
fn closure_captures_cell_and_upvalue() {
    let src = "function f() { var x = 1; var s = 0; return function g() { x = x + 1; \
               return function h() { s = s + x; return x; }; }; }";
    let ir = emit_ir(src);
    let f = &ir.nested[0];
    assert!(f.cells_needed >= 1);
    assert!(count_op(&f.insts, OpCode::MAKE_CELL) >= 1);
    let g = &f.nested[0];
    let g_cells: HashMap<&str, u8> = g.upvalue_captures.iter().map(|c| (c.name.as_str(), c.cell_idx)).collect();
    let mut expected: HashMap<&str, u8> = HashMap::new();
    expected.insert("s", 0);
    expected.insert("x", 1);
    assert_eq!(g_cells, expected, "g 的捕获应按名排序分配 cell 下标");
    assert!(count_op(&g.insts, OpCode::LOAD_UPVALUE) >= 1);
    assert!(count_op(&g.insts, OpCode::STORE_UPVALUE) >= 1);
    let h = &g.nested[0];
    let h_names: HashSet<&str> = h.upvalue_captures.iter().map(|c| c.name.as_str()).collect();
    let mut expected_names: HashSet<&str> = HashSet::new();
    expected_names.insert("s");
    expected_names.insert("x");
    assert_eq!(h_names, expected_names);
    assert!(
        h.upvalue_captures.iter().all(|c| c.parent_uv_idx.is_some()),
        "链式捕获应经父闭包 upvalue 数组取 cell"
    );
}

/// 面 5：对象 rest 展开与数组 holes 的现状形态：REST_OBJECT 的 ext 键是排除键
/// （显式绑定的键名）；解构赋值经 for-of 迭代面（FOR_OF_INIT）逐元素绑定，
/// 空洞元素被消费但不绑定；常量池含 Undefined。
#[test]
fn destructured_rest_and_holes() {
    let src = "var {a, ...r} = {a: 1, b: 2}; var [x, , z] = [1, 2, 3]; \
               var p = [[\"k\", 1]]; for (var [k, v] of p) {}";
    let ir = emit_ir(src);
    let rest = ir
        .insts
        .iter()
        .find(|i| i.op == OpCode::REST_OBJECT)
        .expect("REST_OBJECT 应存在");
    assert_eq!(rest.ext.len(), 1);
    assert_eq!(
        ir.constants[rest.ext[0] as usize],
        Constant::String("a".to_string()),
        "REST_OBJECT 排除键应为显式绑定的键名"
    );
    assert!(count_op(&ir.insts, OpCode::FOR_OF_INIT) >= 1);
    assert!(ir.constants.iter().any(|c| matches!(c, Constant::Undefined)), "常量池应含 Undefined");
}

/// 面 6：模板字面量的 TEMPLATE_STR 面与 quasis 池键（键经 string_forge 编码，
/// 纯 ASCII 键按值还原即原形态）。
#[test]
fn template_literal_pool_keys() {
    let src = "\"use strict\";\nconst t = `a${1}b`;\nconst tag = (s) => s[0];\ntag`x${2}y`";
    let ir = emit_ir(src);
    assert!(count_op(&ir.insts, OpCode::TEMPLATE_STR) >= 1);
    let pool = pool_strings(&ir);
    assert!(pool.contains("a") && pool.contains("b"), "quasis 池键按值应含 a/b: {pool:?}");
}

/// 面 7：类构造器/派生标志、home object 与私有名指令面。
#[test]
fn class_nested_flags() {
    let src = "class B { m() { return 1; } } class A extends B { #p = 2; static s = 3; \
               m() { return this.#p; } }";
    let ir = emit_ir(src);
    let (mut base, mut derived, mut home) = (false, false, false);
    class_flags_scan(&ir, &mut base, &mut derived, &mut home);
    assert!(base, "基类构造器应标 is_class_constructor");
    assert!(derived, "extends 构造器应标 is_derived_constructor");
    assert!(home, "原型方法应标 needs_home_object");
    assert!(tree_count(&ir, OpCode::INIT_PRIVATE) >= 1, "私有字段初始化面应存在");
    assert!(tree_count(&ir, OpCode::GET_PRIVATE) >= 1, "私有成员读面应存在");
}

/// 面 8：严格模式标志在函数体内与脚本顶层的继承口径。
#[test]
fn strict_flag_inherited() {
    let ir = emit_ir("function f() { \"use strict\"; } function g() { return 1; }");
    let by_name = |n: &str| -> &IRFunction {
        ir.nested
            .iter()
            .find(|f| f.function_name.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("nested 缺 {n}"))
    };
    assert!(by_name("f").is_strict, "函数体内 use strict 应置位");
    assert!(!by_name("g").is_strict, "无 directive 函数不应继承严格");
    let top = emit_ir("\"use strict\"; var x = 1;");
    assert!(top.is_strict, "脚本顶层 use strict 应置位");
}

/// 面 9：source_encoded 口径下正则源池键为 marker 形态（与静态口径的明文键不同形）。
#[test]
fn source_encoded_regex_pool_key() {
    let src = "var r = /\\ud800/g;";
    let encode_ir = {
        let alloc = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&alloc, src).unwrap();
        Emitter::new()
            .with_source_encoded(true)
            .emit_program(&program, false, false)
            .unwrap()
    };
    let static_ir = emit_ir(src);
    let enc = pool_strings(&encode_ir);
    assert!(enc.contains("\u{FFFD}d800"), "编码源池键应为 FFFD+hex marker 形态: {enc:?}");
    assert!(!enc.contains("\\ud800"));
    assert!(pool_strings(&static_ir).contains("\\ud800"), "静态源池键为明文转义文本");
}
