//! 闭包捕获分析，单入口收集：对同一 AST（body_stmts + extra_exprs）单趟产出
//! 两个输出（captured_bindings 的 BTreeMap 与 upvalue_captures 的 Vec）。
//! 顶层两个薄包装 `collect_captured_bindings` / `collect_upvalue_names` 签名不变，
//! 调用点不动。

use std::collections::{BTreeMap, HashSet};

use oxide_bytecode::module::UpvalueCapture;
use oxide_parser::{Expression, Statement};

use super::scanner::{collect_capture_names_expr, collect_capture_names_shadowed, CaptureEngine};

/// 单入口：对同一 AST 单趟产出两个输出（captured_bindings 的 BTreeMap 与
/// upvalue_captures 的 Vec）。`own` 为父层捕获判定引用集（深度 ≥ 1 生效），
/// `parent_captured` 与 `parent_upvalues` 为父层两映射（构建全深度引用集），
/// `sub_own` 为 upvalue 判定的显式排除集。
pub(crate) fn collect_captures_and_upvalues(
    body_stmts: &[Statement],
    extra_exprs: &[&Expression],
    own: &HashSet<String>,
    parent_captured: &BTreeMap<String, u8>,
    parent_upvalues: &[UpvalueCapture],
    sub_own: &HashSet<String>,
) -> (BTreeMap<String, u8>, Vec<UpvalueCapture>) {
    // 构建全深度引用集：父捕获名 ∪ 父 upvalue 名。
    let mut parent_names: HashSet<String> = parent_captured.keys().cloned().collect();
    for u in parent_upvalues {
        parent_names.insert(u.name.clone());
    }

    // 双输出累加器。
    let mut captured_out: HashSet<String> = HashSet::new();
    let mut upvalue_out: HashSet<String> = HashSet::new();

    // 构造引擎并扫描。
    let mut eng = CaptureEngine::new(own, &parent_names, sub_own, &mut captured_out, &mut upvalue_out);
    collect_capture_names_shadowed(body_stmts, &mut eng);
    for expr in extra_exprs {
        collect_capture_names_expr(expr, &mut eng);
    }

    // 输出整形：captured_out 排序后按枚举序分配 cell_idx 成 BTreeMap。
    let mut captured_sorted: Vec<String> = captured_out.into_iter().collect();
    captured_sorted.sort();
    let captured_map: BTreeMap<String, u8> =
        captured_sorted.into_iter().enumerate().map(|(i, n)| (n, i as u8)).collect();

    // 输出整形：upvalue_out 排序后逐名取 cell_idx（父捕获映射）与 parent_uv_idx
    // （父 upvalue 向量位置）成 Vec<UpvalueCapture>。
    let mut upvalue_sorted: Vec<String> = upvalue_out.into_iter().collect();
    upvalue_sorted.sort();
    let upvalue_vec: Vec<UpvalueCapture> = upvalue_sorted
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
        .collect();

    (captured_map, upvalue_vec)
}

/// 薄包装：父层捕获判定。`collect_captured_bindings(stmts, extra, own)` =
/// 入口(own, 空父映射).0。签名不变，调用点不动。
pub(crate) fn collect_captured_bindings(
    stmts: &[Statement], extra_exprs: &[&Expression], own: &HashSet<String>,
) -> BTreeMap<String, u8> {
    let empty_map: BTreeMap<String, u8> = BTreeMap::new();
    let empty_vec: Vec<UpvalueCapture> = Vec::new();
    let empty_set: HashSet<String> = HashSet::new();
    collect_captures_and_upvalues(stmts, extra_exprs, own, &empty_map, &empty_vec, &empty_set).0
}
