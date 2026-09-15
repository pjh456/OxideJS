//! 闭包捕获分析（AST 级，emit 前完成，时序无关）。
//!
//! 收集本函数绑定的名字（own_bindings）与被嵌套函数捕获的名字
//! （captured_bindings → cell_idx，按名排序稳定跨 run），为 MAKE_CELL /
//! CELL_GET / CELL_SET 与子函数 upvalue cell_idx 提供统一依据。
//!
//! 五区严格单向分层（本文件 ← names ← scanner ← captured ← upvalues）：
//! 跨区调用单向、无共享局部状态、无接收者状态。

mod captured;
mod names;
mod scanner;
mod upvalues;

use std::collections::HashSet;

pub(crate) use captured::collect_captured_bindings;
pub(crate) use names::collect_own_binding_names;
pub(crate) use names::collect_top_level_function_names;
pub(crate) use names::collect_top_level_function_names_ordered;
pub(crate) use names::collect_var_binding_names;
pub(crate) use upvalues::collect_upvalue_names;

pub(crate) fn collect_fn_param_names(params: &oxide_parser::FormalParameters) -> HashSet<String> {
    let mut names = HashSet::new();
    for p in &params.items {
        collect_binding_pattern_names(&p.pattern, &mut names);
    }
    // rest 形参也是本函数作用域绑定，遮蔽外层同名变量。
    if let Some(rest) = &params.rest {
        collect_binding_pattern_names(&rest.rest.argument, &mut names);
    }
    names
}

/// 递归收集 binding pattern 内全部绑定标识符名（解构 `[a, b]` / `{x: y}` 嵌套）。
pub(crate) fn collect_binding_pattern_names(pattern: &oxide_parser::BindingPattern, out: &mut HashSet<String>) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
            out.insert(bi.name.to_string());
        }
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_binding_pattern_names(p, out);
            }
            if let Some(rest) = &ap.rest {
                collect_binding_pattern_names(&rest.argument, out);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                collect_binding_pattern_names(&prop.value, out);
            }
            if let Some(rest) = &op.rest {
                collect_binding_pattern_names(&rest.argument, out);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_binding_pattern_names(&ap.left, out);
        }
    }
}

/// 收集 for-in/for-of 头部声明（`var x` / 解构 pattern）绑定的名字。
pub(crate) fn collect_for_left_decl_names(left: &oxide_parser::ForStatementLeft, out: &mut HashSet<String>) {
    if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = left {
        for d in &vd.declarations {
            collect_binding_pattern_names(&d.id, out);
        }
    }
}
