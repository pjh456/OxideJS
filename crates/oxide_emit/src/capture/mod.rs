//! 闭包捕获分析（AST 级，emit 前完成，时序无关）。
//!
//! 收集本函数绑定的名字（own_bindings）与被嵌套函数捕获的名字
//! （captured_bindings → cell_idx；cell_idx 按名字排序分配，同一程序重编译得到
//! 相同索引，保证父 MAKE_CELL 与子函数 upvalue 引用同一 cell），为 MAKE_CELL /
//! CELL_GET / CELL_SET 与子函数 upvalue cell_idx 提供统一依据。
//!
//! 四区严格单向分层（本文件 ← names ← scanner ← upvalues）：
//! 跨区调用单向、无共享局部状态、无接收者状态。

mod names;
mod scanner;
mod upvalues;

use std::collections::{HashMap, HashSet};

pub(crate) use names::collect_fn_scope_names;
pub(crate) use names::FnScopeNames;
pub(crate) use upvalues::collect_captured_bindings;
pub(crate) use upvalues::collect_captures_and_upvalues;

/// 收集形参（含 rest）绑定的名字写入影子计次映射（计次加一），
/// 作为本函数作用域的遮蔽集。
pub(crate) fn collect_fn_param_names_into(params: &oxide_parser::FormalParameters, shadow: &mut HashMap<String, u32>) {
    for p in &params.items {
        collect_binding_pattern_names_into(&p.pattern, shadow);
    }
    // rest 形参也是本函数作用域绑定，遮蔽外层同名变量。
    if let Some(rest) = &params.rest {
        collect_binding_pattern_names_into(&rest.rest.argument, shadow);
    }
}

/// 递归收集 binding pattern 内全部绑定标识符名（解构 `[a, b]` / `{x: y}` 嵌套），
/// 写入影子计次映射（计次加一），作用域进入时遮蔽外层同名绑定。
pub(crate) fn collect_binding_pattern_names_into(
    pattern: &oxide_parser::BindingPattern, shadow: &mut HashMap<String, u32>,
) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
            *shadow.entry(bi.name.as_str().to_string()).or_insert(0) += 1;
        }
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_binding_pattern_names_into(p, shadow);
            }
            if let Some(rest) = &ap.rest {
                collect_binding_pattern_names_into(&rest.argument, shadow);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                collect_binding_pattern_names_into(&prop.value, shadow);
            }
            if let Some(rest) = &op.rest {
                collect_binding_pattern_names_into(&rest.argument, shadow);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_binding_pattern_names_into(&ap.left, shadow);
        }
    }
}

/// 递归收集 binding pattern 内全部绑定标识符名并解除影子计次映射（计次减一，
/// 归零删项），作用域退出时解除遮蔽，与 `collect_binding_pattern_names_into` 配对。
fn collect_binding_pattern_names_remove(pattern: &oxide_parser::BindingPattern, shadow: &mut HashMap<String, u32>) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
            shadow_dec(shadow, bi.name.as_str().to_string());
        }
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_binding_pattern_names_remove(p, shadow);
            }
            if let Some(rest) = &ap.rest {
                collect_binding_pattern_names_remove(&rest.argument, shadow);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                collect_binding_pattern_names_remove(&prop.value, shadow);
            }
            if let Some(rest) = &op.rest {
                collect_binding_pattern_names_remove(&rest.argument, shadow);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_binding_pattern_names_remove(&ap.left, shadow);
        }
    }
}

/// 影子计次映射中某名字计次减一，归零删项。
pub(crate) fn shadow_dec(shadow: &mut HashMap<String, u32>, name: String) {
    if let Some(count) = shadow.get_mut(&name) {
        *count -= 1;
        if *count == 0 {
            shadow.remove(&name);
        }
    }
}

/// 收集 for-in/for-of 头部声明（`var x` / 解构 pattern）绑定的名字写入影子计次
/// 映射（计次加一），作用域进入时遮蔽外层同名绑定。
pub(crate) fn collect_for_left_decl_names_into(
    left: &oxide_parser::ForStatementLeft, shadow: &mut HashMap<String, u32>,
) {
    if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = left {
        for d in &vd.declarations {
            collect_binding_pattern_names_into(&d.id, shadow);
        }
    }
}

/// 收集 for-in/for-of 头部声明绑定的名字并解除影子计次映射（计次减一，归零删项），
/// 作用域退出时解除遮蔽，与 `collect_for_left_decl_names_into` 配对。
pub(crate) fn collect_for_left_decl_names_remove(
    left: &oxide_parser::ForStatementLeft, shadow: &mut HashMap<String, u32>,
) {
    if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = left {
        for d in &vd.declarations {
            collect_binding_pattern_names_remove(&d.id, shadow);
        }
    }
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
