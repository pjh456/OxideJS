//! 脚本顶层形态的 IR 级结构断言：parse → `emit_program` 为止，钉死现状 emit 行为。
//!
//! 结构断言顺序无关：只表达 存在 / 计数 / 名字集 / 键名集。GDI 序言对组与 var 入口
//! MAKE_CELL 组的发射序已按名排序固定，但断言仍只取键名集，不依赖寄存器号
//! 与 `n_registers` 绝对值；发射序确定性由 `emission_order_stable_across_compiles`
//! 单独锁定。

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

/// 字节码指纹：`CompiledModule` 未派生 `Debug`，逐字段展开为确定串（子模块递归）。
///
/// 覆盖字节码位模式、常量池、寄存器布局、内置槽映射与闭包捕获描述。
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

/// 同进程连续编译 src 八次，逐次比对 IR 与字节码指纹，断言全等。
fn assert_emission_stable(src: &str, is_eval_script: bool) {
    let mut fingerprints: Vec<(String, String)> = Vec::new();
    for _ in 0..8 {
        let alloc = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&alloc, src).unwrap_or_else(|e| panic!("parse 失败 {src:?}: {e:?}"));
        let ir = Emitter::new()
            .emit_program(&program, false, is_eval_script)
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

/// 发射序确定性：同进程多次编译，GDI 序言、函数入口 MAKE_CELL 的指令序及
/// 常量池/内置槽映射/字节码逐位不随 HashSet 随机迭代序漂移。
#[test]
fn emission_order_stable_across_compiles() {
    // 多个顶层 var 驱动 GDI 序言；函数内多个被捕获 var 驱动函数入口 MAKE_CELL。
    assert_emission_stable(
        "var b=1,a=2,d=3,c=4; function f(){ var z=1,y=2; return function(){ return z+y+a; }; } f();",
        false,
    );
    // eval 顶层函数声明撞多个不可写全局内置名时，抛错消息名选择按源序稳定。
    assert_emission_stable("function Infinity(){} function NaN(){}", true);
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

/// 面 13：直接子函数（深度 0）形参默认值对父级绑定的直接引用须纳入父捕获集
/// （父建 cell、子建 upvalue），否则逃逸闭包缺参调用经 LOAD_VAR 读父帧死寄存器取陈旧值。
#[test]
fn nested_fn_default_ref_captures_parent_binding() {
    let src = "function f() { var x = 1; var g = function(a = x) { return a; }; return g; }";
    let ir = emit_ir(src);
    let f = &ir.nested[0];
    assert!(f.cells_needed >= 1, "父函数应为默认值引用的 x 建 cell");
    assert!(count_op(&f.insts, OpCode::MAKE_CELL) >= 1, "父函数入口应发 MAKE_CELL");
    let g = &f.nested[0];
    let g_names: HashSet<&str> = g.upvalue_captures.iter().map(|c| c.name.as_str()).collect();
    assert!(g_names.contains("x"), "子函数形参默认值引用的 x 应成为子 upvalue");
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
/// 纯 ASCII 键按值还原即原形态）。全字面量模板已折单常量（见字符串域折叠
/// 测试），此处用非字面量内插表达式保持 TEMPLATE_STR 路径。
#[test]
fn template_literal_pool_keys() {
    let src = "\"use strict\";\nvar x = 1;\nconst t = `a${x}b`;\nconst tag = (s) => s[0];\ntag`x${2}y`";
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

/// 面 9：对象字面量方法挂 home object（模块标 needs_home_object 且树内存在
/// SET_HOME_OBJECT），计算型 super 成员发 SUPER_GET_PROP 且 this 装载先于键表达式。
#[test]
fn object_method_home_object_and_computed_super() {
    let src = "var o = { m() { return super.x; }, g() { return super[k]; } };";
    let ir = emit_ir(src);
    let (mut _base, mut _derived, mut home) = (false, false, false);
    class_flags_scan(&ir, &mut _base, &mut _derived, &mut home);
    assert!(home, "对象方法应标 needs_home_object");
    assert!(tree_count(&ir, OpCode::SET_HOME_OBJECT) >= 2, "对象方法应挂 SET_HOME_OBJECT");
    assert!(tree_count(&ir, OpCode::SUPER_GET_PROP) >= 2, "super 读应发 SUPER_GET_PROP");

    // 计算型 super 成员：this 装载（LOAD_VAR This）先于键表达式求值（规范序）。
    let m_body = ir
        .nested
        .iter()
        .find(|f| f.function_name.as_deref() == Some("g"))
        .expect("方法 g 应存在");
    let this_pos = m_body
        .insts
        .iter()
        .position(|i| i.op == OpCode::LOAD_VAR && matches!(i.a, Operand::This))
        .expect("应存在 LOAD_VAR This");
    let super_pos = m_body
        .insts
        .iter()
        .position(|i| i.op == OpCode::SUPER_GET_PROP)
        .expect("应存在 SUPER_GET_PROP");
    assert!(this_pos < super_pos, "this 装载应先于 SUPER_GET_PROP");
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

/// 面 9：未捕获 `var` 名在函数入口补发 undefined 定义（预声明只登记槽位，
/// 声明点对已绑定名跳过 undefined 写，入口缺定义会使首次写入前读取取到残留）。
#[test]
fn var_entry_init_emits_undefined_store() {
    let ir = emit_ir("function f() { var x; return typeof x; }");
    let f = &ir.nested[0];
    assert_eq!(count_op(&f.insts, OpCode::STORE_VAR), 1, "未捕获 var 应在函数入口恰写一次 undefined");
}

/// 面 10：入口补写的过滤面——形参、`arguments`、捕获名不触发补写，
/// 多个未捕获 var 名各自补一条。
#[test]
fn var_entry_init_filters_params_arguments_captured() {
    let param = emit_ir("function f(a) { var x; return typeof a + typeof x; }");
    assert_eq!(count_op(&param.nested[0].insts, OpCode::STORE_VAR), 1, "形参名不补写，仅未捕获 var 补写");

    let arguments = emit_ir("function f() { var x; return arguments.length + typeof x; }");
    assert_eq!(
        count_op(&arguments.nested[0].insts, OpCode::STORE_VAR),
        1,
        "arguments 名不补写，未捕获 var 补写"
    );

    let captured = emit_ir("function f() { var x; return function g() { return typeof x; }; }");
    assert_eq!(
        count_op(&captured.nested[0].insts, OpCode::STORE_VAR),
        0,
        "捕获名由入口 MAKE_CELL(undefined) 实例化，不再补 STORE_VAR"
    );

    let multi = emit_ir("function f() { var a, b; return typeof a + typeof b; }");
    assert_eq!(count_op(&multi.nested[0].insts, OpCode::STORE_VAR), 2, "多 var 名各自补一条入口定义");
}

/// 面 11：source_encoded 口径下正则源池键为 marker 形态（与静态口径的明文键不同形）。
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

/// 面 12：块函数抑制集含解构形参叶名——块函数名撞叶名时不建外层 var 绑定、
/// 声明点不写回（STORE_VAR 少一条写回）；非抑制名块函数有入口 undefined 写
/// 与声明点写回。
#[test]
fn block_fn_suppressed_by_destructured_param_leaf() {
    // 撞解构叶名：叶名绑定（1）+ 块函数物化自写（1）= 2，无外层 var 写回。
    let suppressed = emit_ir("function f({x}) { { function x() { return 1; } } return 2; }");
    let f = &suppressed.nested[0];
    assert_eq!(count_op(&f.insts, OpCode::STORE_VAR), 2, "撞解构叶名的块函数被抑制：无外层 var 写回");

    // 非抑制对照：入口 undefined 写（1）+ 块函数物化自写（1）+ 外层 var 写回（1）= 3。
    let control = emit_ir("function f() { { function y() { return 1; } } return 2; }");
    let fc = &control.nested[0];
    assert_eq!(
        count_op(&fc.insts, OpCode::STORE_VAR),
        3,
        "非抑制块函数应有入口 undefined 写与外层 var 写回"
    );
}

/// 面 22：Annex B.3.3.3 早期错误跳过——sloppy eval 代码内块函数名撞同树词法
/// 声明时编译不报重复声明错，且函数声明不覆写词法绑定（STORE_VAR 少一条写回）。
#[test]
fn eval_early_error_skip_no_binding() {
    // base 形：碰撞在 eval 顶层 let。编译成功且函数不覆写词法绑定。
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, "let f = 123; if (true) function f() {}").unwrap();
    let ir = Emitter::new()
        .emit_program(&program, false, true)
        .unwrap_or_else(|e| panic!("base 形应编译成功：{e}"));
    // `let f = 123` 初始化写（1）+ 函数声明不覆写词法绑定（0）= 1。
    assert_eq!(
        count_op(&ir.insts, OpCode::STORE_VAR),
        1,
        "早期错误跳过：函数声明不覆写词法绑定"
    );

    // -block 形：碰撞在同块 let。编译不报「Identifier has already been declared」。
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, "{ let f = 123; if (true) function f() {} }").unwrap();
    let _ir = Emitter::new()
        .emit_program(&program, false, true)
        .unwrap_or_else(|e| panic!("-block 形应编译成功：{e}"));
}

/// 面 14：双常量折叠入口结构回归——双字面量算术编译后目标操作码计数为 0、
/// 常量池含折值，锁死折叠入口不随后续重构丢失。
#[test]
fn binary_literal_fold_entry() {
    // `2 * 3` 乘法编码恒为 f64 → Number(6.0)；`1 + 2` 为 int+int → Int(3)。
    // 两操作数均为直接字面量才折（复合子表达式如 `1 + 2 * 3` 的外层加法不折）。
    let ir = emit_ir("var a = 2 * 3; var b = 1 + 2;");
    assert_eq!(count_op(&ir.insts, OpCode::MUL), 0, "双字面量乘法应折叠，MUL 计数为 0");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0, "双字面量加法应折叠，ADD 计数为 0");
    assert!(ir.constants.contains(&Constant::Number(6.0)), "常量池应含乘法折值 Number(6.0)");
    assert!(ir.constants.contains(&Constant::Int(3)), "常量池应含加法折值 Int(3)");
}

/// 面 15：字符串域链折叠命中矩阵——全操作数为可折叠字面量且至少一个字符串
/// 字面量时，Addition 链折单字符串常量：ADD/CONCAT_N 计数为 0、池含折值键。
#[test]
fn string_chain_fold_hit() {
    // 纯字符串链：池含 "ab"。
    let ir = emit_ir("var s = \"a\" + \"b\";");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0, "全字面量字符串链应折叠，ADD 计数为 0");
    assert_eq!(count_op(&ir.insts, OpCode::CONCAT_N), 0, "全字面量字符串链应折叠，CONCAT_N 计数为 0");
    assert!(pool_strings(&ir).contains("ab"), "池应含折值 ab");

    // 字符串加数字加布尔加 null：池含 "a1truenull"。
    let ir = emit_ir("var s = \"a\" + 1 + true + null;");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    assert_eq!(count_op(&ir.insts, OpCode::CONCAT_N), 0);
    assert!(pool_strings(&ir).contains("a1truenull"), "池应含折值 a1truenull");

    // 字符串加 BigInt：池含 "x10"。
    let ir = emit_ir("var s = \"x\" + 10n;");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    assert!(pool_strings(&ir).contains("x10"), "池应含折值 x10");

    // 字符串加非整数字面量（Number 臂走 js_number_to_string）：池含 "a1.5"。
    let ir = emit_ir("var s = \"a\" + 1.5;");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    assert!(pool_strings(&ir).contains("a1.5"), "池应含折值 a1.5");

    // 字符串加 -0（is_int_literal 为真走 Int 臂）：池含 "a0"。
    let ir = emit_ir("var s = \"a\" + -0;");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    assert!(pool_strings(&ir).contains("a0"), "池应含折值 a0");

    // 孤立 surrogate 字符串段：池键为 FFFD+hex4 marker 形态加 "a"。
    let ir = emit_ir("var s = \"\\ud800\" + \"a\";");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    let pool = pool_strings(&ir);
    assert!(pool.contains("\u{FFFD}d800a"), "池键应为 FFFD+hex4 marker 形态加 a: {pool:?}");

    // 超平面字符段：经 encode_key 良配对形态入池。
    let ir = emit_ir("var s = \"\\u{1F600}\" + \"a\";");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 0);
    assert!(pool_strings(&ir).contains("\u{1F600}a"), "池应含超平面字符良配对形态加 a");
}

/// 面 16：字符串域链不折矩阵——含标识符操作数或全数字链不落字符串域，
/// 既有 CONCAT_N/ADD 路径计数不变、池不含折值键。
#[test]
fn string_chain_fold_miss() {
    // 标识符操作数（两操作数形态）：不折，落回左结合 ADD。
    let ir = emit_ir("var x = 1; var s = x + \"a\";");
    assert_eq!(count_op(&ir.insts, OpCode::ADD), 1, "含标识符的链应落回 ADD");
    assert_eq!(count_op(&ir.insts, OpCode::CONCAT_N), 0);
    assert!(!pool_strings(&ir).contains("xa"), "含标识符的链不应折出字符串键");

    // 全数字链：数值域守卫排除，不折出字符串键，落回 CONCAT_N。
    let ir = emit_ir("var s = 1 + 2 + 3;");
    assert_eq!(count_op(&ir.insts, OpCode::CONCAT_N), 1, "全数字链应落回 CONCAT_N");
    assert!(!pool_strings(&ir).contains("123"), "全数字链不应折出字符串键 123");

    // 字符串夹标识符（三操作数形态）：不折，落回 CONCAT_N。
    let ir = emit_ir("var x = 1; var s = \"a\" + x + \"b\";");
    assert_eq!(count_op(&ir.insts, OpCode::CONCAT_N), 1, "含标识符的链应落回 CONCAT_N");
    assert!(!pool_strings(&ir).contains("axb"), "含标识符的链不应折出字符串键");
}

/// 面 17：模板字面量折叠命中矩阵——全内插表达式为可折叠字面量且全部 cooked
/// 为 Some 时折单字符串常量：TEMPLATE_STR 计数为 0、池含折值键。
#[test]
fn template_fold_hit() {
    let ir = emit_ir("var t = `a${1}b`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 0, "全字面量模板应折叠，TEMPLATE_STR 计数为 0");
    assert!(pool_strings(&ir).contains("a1b"), "池应含折值 a1b");

    // 内插 BigInt：池含 "x10y"。
    let ir = emit_ir("var t = `x${10n}y`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 0);
    assert!(pool_strings(&ir).contains("x10y"), "池应含折值 x10y");

    // 无内插表达式的单 quasi 模板：折单串。
    let ir = emit_ir("var t = `hello`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 0);
    assert!(pool_strings(&ir).contains("hello"), "池应含折值 hello");

    // 孤立 surrogate quasi：池键为 marker 形态。
    let ir = emit_ir("var t = `\\ud800${1}`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 0);
    let pool = pool_strings(&ir);
    assert!(pool.contains("\u{FFFD}d8001"), "池键应为 FFFD+hex4 marker 形态加 1: {pool:?}");
}

/// 面 18：模板字面量不折矩阵——内插表达式非字面量时落回 TEMPLATE_STR；
/// 带标签模板走 GET_TEMPLATE_OBJECT 加 CALL 的独立路径，不经折叠。
#[test]
fn template_fold_miss() {
    // 表达式非字面量：TEMPLATE_STR 计数 1，池不含折值键。
    let ir = emit_ir("var x = 1; var t = `a${x}b`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 1, "非字面量内插应落回 TEMPLATE_STR");
    assert!(!pool_strings(&ir).contains("axb"), "不折时池不应含折值键");

    // 带标签模板：走 GET_TEMPLATE_OBJECT 加 CALL，不折。
    let ir = emit_ir("var tag = (s) => s[0]; var t = tag`a${1}`;");
    assert_eq!(count_op(&ir.insts, OpCode::TEMPLATE_STR), 0, "带标签模板不发 TEMPLATE_STR");
    assert_eq!(count_op(&ir.insts, OpCode::GET_TEMPLATE_OBJECT), 1, "带标签模板应发 GET_TEMPLATE_OBJECT");
    assert_eq!(count_op(&ir.insts, OpCode::CALL), 1, "带标签模板应发 CALL");
}

/// 面 19：字符串域折叠发射确定性——折叠路径纯函数无迭代序依赖，链折叠与
/// 模板折叠源串同进程连续编译指纹全等。
#[test]
fn string_fold_emission_stable() {
    assert_emission_stable("var s = \"a\" + 1 + true + null;", false);
    assert_emission_stable("var t = `a${1}b`;", false);
}

/// 按名递归查找嵌套函数（树序）。
fn find_fn_by_name<'a>(ir: &'a IRFunction, name: &str) -> &'a IRFunction {
    find_fn_by_name_opt(ir, name).unwrap_or_else(|| panic!("嵌套函数 {name} 未找到"))
}

fn find_fn_by_name_opt<'a>(ir: &'a IRFunction, name: &str) -> Option<&'a IRFunction> {
    ir.nested.iter().find_map(|f| {
        if f.function_name.as_deref() == Some(name) {
            Some(f)
        } else {
            find_fn_by_name_opt(f, name)
        }
    })
}

/// 面 20：类方法内嵌套函数对私有字段赋值目标，目标对象须入捕获集——
/// 读路径本就扫对象，赋值目标臂同口径；不捕获则运行期回落全局对象，
/// brand 检查必失败。
#[test]
fn private_field_assign_target_captures_object() {
    let src = "class C { #m = 'Test262'; method() { let self = this; \
               function innerFunction() { self.#m = 'Test262'; } innerFunction(); } }";
    let ir = emit_ir(src);
    let inner = find_fn_by_name(&ir, "innerFunction");
    let names: HashSet<&str> = inner.upvalue_captures.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains("self"), "私有字段赋值目标对象须入捕获集");
    assert!(tree_count(&ir, OpCode::SET_PRIVATE) >= 1, "私有字段写面应存在");
}

/// 面 20 对照：类方法内嵌套函数对私有字段读路径，目标对象已入捕获集（不回归）。
#[test]
fn private_field_read_captures_object() {
    let src = "class C { #m = 'Test262'; method() { let self = this; \
               function innerFunction() { return self.#m; } innerFunction(); } }";
    let ir = emit_ir(src);
    let inner = find_fn_by_name(&ir, "innerFunction");
    let names: HashSet<&str> = inner.upvalue_captures.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains("self"), "私有字段读路径目标对象须入捕获集");
}

/// 面 21：顶层 vreg 号越过 255 后，类静态块 `this` 仍正确读类构造器 vreg——
/// 构造器寄存器不截断、不与 this 槽（254）特判混淆。
#[test]
fn static_block_this_reads_ctor_vreg_beyond_u8() {
    // 大量顶层 var 把 vreg 号推过 255，再声明含静态块的类。
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("var v{i} = {i};\n"));
    }
    src.push_str("class C { static { this.x = 1; } }");
    // 修复前此源码 emit 报「class constructor register … exceeds u8 limit」，
    // `emit_ir` 会 panic。
    let ir = emit_ir(&src);
    // 静态块 `this` 应发 LOAD_VAR 读构造器 vreg（Operand::Reg），非 this 槽。
    let this_load = ir
        .insts
        .iter()
        .filter(|i| i.op == OpCode::LOAD_VAR)
        .find(|i| matches!(i.a, Operand::Reg(_)))
        .expect("静态块 this 应发 LOAD_VAR Reg");
    let Operand::Reg(r) = this_load.a else { unreachable!() };
    assert!(r > 255, "静态块 this 的构造器 vreg 号应越过 255：{r}");
    assert!(
        !ir.insts
            .iter()
            .any(|i| i.op == OpCode::LOAD_VAR && matches!(i.a, Operand::This)),
        "静态块 this 不应走 this 槽"
    );
}
