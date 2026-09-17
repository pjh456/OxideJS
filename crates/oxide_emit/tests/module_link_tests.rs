//! 模块链接面的 IR 级断言：内存版 `ModuleSourceLoader` +
//! `emit_program_module`，钉死 import prelude / export 就地注册 / star 再导出 /
//! 循环导入编译期报错 / 数据模块的现状行为。
//!
//! 路径口径：加载器 `path` 回填表键原样字符串，与入口 `module_path` 入参逐字符
//! 一致（虚拟路径 canonicalize 失败保留原路径），自导入身份比较与循环检测依赖
//! 此口径。断言不依赖寄存器号与指令序。

use std::collections::HashMap;
use std::collections::HashSet;

use oxide_bytecode::opcode::OpCode;
use oxide_emit::module::{ModuleKind, ModuleSourceLoader, ResolvedModule};
use oxide_emit::{Constant, Emitter};
use oxide_ir::IRFunction;

/// 内存版加载器：specifier 精确查表；`path` 原样回填表键。
/// `resolved` 记录每次 resolve 的 (specifier, 属性对)，供链接期观察。
struct MemLoader {
    table: HashMap<String, (ModuleKind, String)>,
    resolved: Vec<(String, Vec<(String, String)>)>,
}

impl ModuleSourceLoader for MemLoader {
    fn resolve(
        &mut self, _base_dir: &str, specifier: &str, attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        self.resolved.push((
            specifier.to_string(),
            attributes.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ));
        self.table
            .get(specifier)
            .cloned()
            .map(|(kind, source)| ResolvedModule {
                source,
                path: specifier.to_string(),
                kind,
            })
            .ok_or_else(|| format!("MemLoader 无 {specifier}"))
    }
}

/// 夹具：parse_module + emit_program_module，返回 IR 与加载器。
fn emit_module(src: &str, path: &str, deps: &[(&str, ModuleKind, &str)]) -> Result<(IRFunction, MemLoader), String> {
    let alloc = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse_module(&alloc, src).map_err(|errs| format!("parse_module 失败 {src:?}: {errs:?}"))?;
    let mut loader = MemLoader {
        table: deps.iter().map(|(s, k, t)| (s.to_string(), (*k, t.to_string()))).collect(),
        resolved: Vec::new(),
    };
    let ir = Emitter::new().emit_program_module(&program, path, &mut loader)?;
    Ok((ir, loader))
}

fn builtin_names(ir: &IRFunction) -> HashSet<String> {
    ir.builtin_reg_map.iter().map(|(n, _)| n.clone()).collect()
}

fn builtin_reg(ir: &IRFunction, name: &str) -> Option<u32> {
    ir.builtin_reg_map.iter().find(|(n, _)| n == name).map(|(_, r)| *r)
}

/// 以某 built-in 为 callee（rd 槽）的 CALL_NATIVE 计数。
fn native_calls_to(ir: &IRFunction, name: &str) -> usize {
    let Some(reg) = builtin_reg(ir, name) else {
        return 0;
    };
    ir.insts
        .iter()
        .filter(|i| i.op == OpCode::CALL_NATIVE && matches!(i.rd, oxide_ir::operand::Operand::Reg(r) if r == reg))
        .count()
}

fn pool_strings(ir: &IRFunction) -> HashSet<String> {
    ir.constants
        .iter()
        .filter_map(|c| if let Constant::String(s) = c { Some(s.clone()) } else { None })
        .collect()
}

/// 在顶层 nested 中按 `function_name` 查找模块。
fn named_module<'a>(ir: &'a IRFunction, name: &str) -> Option<&'a IRFunction> {
    ir.nested.iter().find(|m| m.function_name.as_deref() == Some(name))
}

/// 面 1：import 绑定 prelude 经 `__moduleLinkGet` 链接，依赖模块编入子模块树。
#[test]
fn import_binding_prelude() {
    let (ir, _) = emit_module(
        "import { x } from \"./dep.js\"; var y = x;",
        "./entry.js",
        &[("./dep.js", ModuleKind::Js, "export var x = 42;")],
    )
    .expect("模块编译应成功");
    assert!(builtin_names(&ir).contains("__moduleLinkGet"), "prelude 应链接 __moduleLinkGet");
    assert!(native_calls_to(&ir, "__moduleLinkGet") >= 1, "命名导入应发 __moduleLinkGet 调用");
    assert_eq!(ir.nested.len(), 1, "依赖模块应编入子模块树");
    let dep = &ir.nested[0];
    assert!(!dep.insts.is_empty());
    assert!(builtin_names(dep).contains("__moduleSet"), "依赖的 export 应就地注册");
}

/// 面 2：export 声明就地注册导出值到命名空间（`__moduleSet` 逐名成对）。
#[test]
fn export_in_place_register() {
    let (ir, _) = emit_module("export function f() {} export const c = 2;", "./entry.js", &[]).expect("模块编译应成功");
    assert!(builtin_names(&ir).contains("__moduleSet"));
    assert_eq!(native_calls_to(&ir, "__moduleSet"), 2, "f 与 c 各注册一次");
    let pool = pool_strings(&ir);
    assert!(pool.contains("f") && pool.contains("c"), "导出名应成对入池: {pool:?}");
}

/// 面 3：`export * from` 匿名 star 再导出经 `__moduleStar` 整体转发（现状口径）。
#[test]
fn export_all_from() {
    let (ir, _) = emit_module(
        "export * from \"./dep.js\";",
        "./entry.js",
        &[("./dep.js", ModuleKind::Js, "export var z = 1;")],
    )
    .expect("star 再导出应编译通过");
    assert!(builtin_names(&ir).contains("__moduleStar"), "匿名 star 应走 __moduleStar");
    assert_eq!(ir.nested.len(), 1);
}

/// 面 4：循环导入在编译期报错，错误文本即循环检测点产物。
#[test]
fn circular_import_compile_error() {
    let src_a = "import { y } from \"./b.js\"; export var x = 1;";
    let result = emit_module(
        src_a,
        "./a.js",
        &[
            ("./a.js", ModuleKind::Js, src_a),
            ("./b.js", ModuleKind::Js, "import { x } from \"./a.js\"; export var y = 2;"),
        ],
    );
    assert_eq!(
        result.err().as_deref(),
        Some("circular module import not supported: ./a.js"),
        "Err 应来自循环检测点（自导入身份比较命中）"
    );
}

/// 面 5：json 数据模块经 `__moduleData` 物化，数据源文本入子模块常量池。
#[test]
fn json_data_module() {
    let json = r#"{"k":1}"#;
    let (ir, loader) = emit_module(
        "import data from \"./d.json\" assert { type: \"json\" }; data;",
        "./entry.js",
        &[("./d.json", ModuleKind::Json, json)],
    )
    .expect("数据模块编译应成功");
    // import 属性应原样传给加载器。
    let attrs = loader
        .resolved
        .iter()
        .find(|(s, _)| s == "./d.json")
        .map(|(_, a)| a.clone())
        .expect("d.json 应被 resolve");
    assert_eq!(attrs, vec![("type".to_string(), "json".to_string())]);
    assert_eq!(ir.nested.len(), 1);
    let data = &ir.nested[0];
    assert!(builtin_names(data).contains("__moduleData"), "数据模块应链接 __moduleData");
    let pool = pool_strings(data);
    assert!(pool.contains("json") && pool.contains(json), "json 源文本应入池: {pool:?}");
}

/// 面 6：`export default` 匿名函数/生成器/箭头/类经 SetFunctionName 落隐式名 "default"。
/// 带方法的类必须命中构造器，而不是构造器之后 push 的方法子模块。
#[test]
fn export_default_anonymous_implicit_name() {
    let (ir, _) = emit_module("export default function() {}", "./entry.js", &[]).expect("匿名函数声明应编译成功");
    assert!(named_module(&ir, "default").is_some(), "匿名函数声明应落隐式名");

    let (ir, _) = emit_module("export default function* () {}", "./entry.js", &[]).expect("匿名生成器声明应编译成功");
    assert!(named_module(&ir, "default").is_some_and(|m| m.is_generator), "匿名生成器应落隐式名");

    let (ir, _) = emit_module("export default (function() {})", "./entry.js", &[]).expect("括号函数表达式应编译成功");
    assert!(named_module(&ir, "default").is_some(), "匿名函数表达式应落隐式名");

    let (ir, _) = emit_module("export default (() => {})", "./entry.js", &[]).expect("箭头表达式应编译成功");
    assert!(named_module(&ir, "default").is_some_and(|m| m.is_arrow), "匿名箭头应落隐式名");

    let (ir, _) = emit_module("export default class { m() {} }", "./entry.js", &[]).expect("匿名类声明应编译成功");
    let ctor = ir.nested.iter().find(|m| m.is_class_constructor).expect("类构造器应在 nested");
    assert_eq!(ctor.function_name.as_deref(), Some("default"), "匿名类构造器应落隐式名");
    assert!(named_module(&ir, "m").is_some(), "方法模块应保留自身名");

    let (ir, _) = emit_module("export default (class { m() {} })", "./entry.js", &[]).expect("括号类表达式应编译成功");
    let ctor = ir.nested.iter().find(|m| m.is_class_constructor).expect("类构造器应在 nested");
    assert_eq!(ctor.function_name.as_deref(), Some("default"), "匿名类表达式构造器应落隐式名");
    assert!(named_module(&ir, "m").is_some(), "方法模块应保留自身名");
}

/// 面 7：`export default` 具名变体与非常量表达式不被隐式名覆盖。
#[test]
fn export_default_implicit_name_named_unchanged() {
    let (ir, _) = emit_module("export default class Foo { m() {} }", "./entry.js", &[]).expect("具名类声明应编译成功");
    let ctor = ir.nested.iter().find(|m| m.is_class_constructor).expect("类构造器应在 nested");
    assert_eq!(ctor.function_name.as_deref(), Some("Foo"));

    let (ir, _) =
        emit_module("export default (class Bar { m() {} })", "./entry.js", &[]).expect("具名类表达式应编译成功");
    let ctor = ir.nested.iter().find(|m| m.is_class_constructor).expect("类构造器应在 nested");
    assert_eq!(ctor.function_name.as_deref(), Some("Bar"));

    let (ir, _) = emit_module("export default (function f() {})", "./entry.js", &[]).expect("具名函数表达式应编译成功");
    assert!(named_module(&ir, "f").is_some(), "具名函数表达式应保留自身名");

    let (ir, _) = emit_module("export default 42", "./entry.js", &[]).expect("字面量应编译成功");
    assert!(named_module(&ir, "default").is_none(), "非函数值不应注入隐式名");
}

/// STORE_VAR 指令计数（合成绑定惰性化守卫用）。
fn store_var_count(ir: &IRFunction) -> usize {
    ir.insts.iter().filter(|i| i.op == OpCode::STORE_VAR).count()
}

/// 面 8：无 self-import 的 default 表达式模块零 diff。
/// `export default 42` 不建合成 `*default*` 绑定，故除 export 注册外无任何
/// STORE_VAR（字面量经 LOAD_CONST 直接注册）。若合成绑定被无条件建立，会多出
/// 一条 STORE_VAR，本守卫即失败。
#[test]
fn default_expression_module_has_no_synthetic_binding() {
    let (ir, _) = emit_module("export default 42", "./entry.js", &[]).expect("字面量应编译成功");
    assert_eq!(store_var_count(&ir), 0, "default 表达式模块不应建合成绑定槽");
    assert_eq!(native_calls_to(&ir, "__moduleSet"), 1, "default 只注册一次导出");
}

/// 面 8b：无 self-import 的 `export let x = 1` 不因 default 面多出槽位/指令。
/// 该模块唯一 STORE_VAR 是 x 自身的声明初始化；零 diff 守卫要求不出现第二个槽。
#[test]
fn named_export_without_self_import_keeps_single_store() {
    let (ir, _) = emit_module("export let x = 1;", "./entry.js", &[]).expect("导出声明应编译成功");
    assert_eq!(store_var_count(&ir), 1, "x 声明只应有一条 STORE_VAR");
    assert_eq!(native_calls_to(&ir, "__moduleSet"), 1, "x 只注册一次导出");
}

/// 面 8c：无 source 再导出 `export { x }` 不引入合成绑定。
/// 局部 x 由普通声明承载，再导出只读值注册；无 self-import 时零额外 STORE_VAR。
#[test]
fn local_reexport_without_self_import_keeps_single_store() {
    let (ir, _) = emit_module("let x = 1; export { x };", "./entry.js", &[]).expect("本地再导出应编译成功");
    assert_eq!(store_var_count(&ir), 1, "x 声明只应有一条 STORE_VAR");
}

/// 面 9：self-import 别名以源绑定槽位承载，导出注册不再回写占位槽。
/// `export default 42` 有 self-import default 时合成绑定存在，但别名与源共享
/// 槽位：export 语句只发一次 LOAD_CONST + `__moduleSet`，无额外的回写 STORE_VAR。
#[test]
fn self_import_default_alias_has_no_placeholder_writeback() {
    let (ir, _) = emit_module(
        "import d from './self.js'; export default 42;",
        "./self.js",
        &[("./self.js", ModuleKind::Js, "export default 42;")],
    )
    .expect("self-import default 应编译成功");
    assert_eq!(native_calls_to(&ir, "__moduleSet"), 1, "default 只注册一次导出");
    // 别名与合成源绑定共享槽位：仅 default 表达式的绑定写入一条 STORE_VAR，
    // 不出现旧占位路径的额外回写。
    assert_eq!(store_var_count(&ir), 1, "不应出现占位回写带来的第二条 STORE_VAR");
}
