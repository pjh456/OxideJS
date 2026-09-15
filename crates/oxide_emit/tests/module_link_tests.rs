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
