//! 闭包捕获分析，子函数 upvalue 分析：子函数体对父被捕获绑定的引用
//! → Vec<UpvalueCapture>（排序对齐 cell_idx，enclosing_reg 留 0 待
//! assemble_ir 填充）。

use std::collections::{BTreeMap, HashSet};

use oxide_bytecode::module::UpvalueCapture;
use oxide_parser::Statement;

use super::scanner::{collect_capture_names_expr, collect_capture_names_shadowed};

/// 分析子函数 body：引用父级被捕获绑定的名字 → upvalue_captures。
/// 名字若在父 `captured_bindings`（父 own cell）则 `cell_idx` 索引定义方表；
/// 若在父 `upvalue_captures`（父自身从更外层捕获）则链式标记 `parent_uv_idx`，
/// 运行时从父闭包 upvalues 取 cell。enclosing_reg 由父 emit 完成后填充。
pub(crate) fn collect_upvalue_names(
    body_stmts: &[Statement], extra_exprs: &[&oxide_parser::Expression], parent_captured: &BTreeMap<String, u8>,
    parent_upvalues: &[UpvalueCapture], sub_own: &HashSet<String>,
) -> Vec<UpvalueCapture> {
    let mut parent_names: HashSet<String> = parent_captured.keys().cloned().collect();
    for u in parent_upvalues {
        parent_names.insert(u.name.clone());
    }
    let mut names = HashSet::new();
    collect_capture_names_shadowed(body_stmts, &parent_names, sub_own, &mut names);
    // 参数默认值表达式（不在 body_stmts）里的嵌套函数引用也会捕获父变量。
    for expr in extra_exprs {
        collect_capture_names_expr(expr, &parent_names, sub_own, &mut names);
    }
    // HashSet 迭代序带随机种子（进程级非确定），必须排序使 upvalue_captures 的顺序与
    // 父 captured_bindings 的 cell_idx（BTreeMap 名字序）对齐——否则 LOAD_UPVALUE 的
    // a 槽 uv_idx 编码与 cell_idx 错位，读错 upvalue。
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let parent_uv_idx = parent_upvalues.iter().position(|u| u.name == name).map(|i| i as u8);
            let cell_idx = parent_captured.get(&name).copied().unwrap_or(0);
            UpvalueCapture {
                name,
                enclosing_reg: 0, // assemble_ir 时从父符号表填充
                cell_idx,
                parent_uv_idx,
            }
        })
        .collect()
}
