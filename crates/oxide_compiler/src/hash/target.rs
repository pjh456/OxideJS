//! 赋值目标（SimpleAssignmentTarget / AssignmentTarget）的结构哈希。

use super::*;

pub(super) fn hash_simple_assignment_target(
    target: &SimpleAssignmentTarget, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    hash_match!(HashDomain::SimpleAssignmentTarget, target, h, {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
            if include_binding_names {
                ident.name.as_str().hash(h);
            }
        }
        SimpleAssignmentTarget::StaticMemberExpression(member) => {
            expression::hash_expression(&member.object, h, include_binding_names);
        }
        SimpleAssignmentTarget::ComputedMemberExpression(member) => {
            expression::hash_expression(&member.object, h, include_binding_names);
            expression::hash_expression(&member.expression, h, include_binding_names);
        }
        SimpleAssignmentTarget::PrivateFieldExpression(member) => {
            expression::hash_expression(&member.object, h, include_binding_names);
            member.field.name.as_str().hash(h);
        }
        SimpleAssignmentTarget::TSAsExpression(ts) => {
            expression::hash_expression(&ts.expression, h, include_binding_names);
        }
        SimpleAssignmentTarget::TSSatisfiesExpression(ts) => {
            expression::hash_expression(&ts.expression, h, include_binding_names);
        }
        SimpleAssignmentTarget::TSNonNullExpression(ts) => {
            expression::hash_expression(&ts.expression, h, include_binding_names);
        }
        SimpleAssignmentTarget::TSTypeAssertion(ts) => {
            expression::hash_expression(&ts.expression, h, include_binding_names);
        }
    });
}

/// 哈希全量 `AssignmentTarget`：简单目标（标识符/成员/TS 断言）复用既有分派，
/// 数组/对象解构目标递归计入元素、键、默认值表达式与 rest 的完整结构。
///
/// # 边界与前提
/// - `include_binding_names` 透传至简单目标、默认值表达式与解构目标内部绑定名；
///   解构目标的结构（元素数、属性键、rest）两种粒度下恒计入。
pub(super) fn hash_assignment_target(
    target: &AssignmentTarget, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    if let Some(simple) = target.as_simple_assignment_target() {
        hash_simple_assignment_target(simple, h, include_binding_names);
        return;
    }
    match target {
        AssignmentTarget::ArrayAssignmentTarget(ap) => {
            hash_array_assignment_target(ap, h, include_binding_names);
        }
        AssignmentTarget::ObjectAssignmentTarget(op) => {
            hash_object_assignment_target(op, h, include_binding_names);
        }
        _ => {}
    }
}

/// 哈希数组赋值目标：元素逐个哈希（省略位以哨兵区分），rest 有无以哨兵区分。
pub(super) fn hash_array_assignment_target(
    target: &ArrayAssignmentTarget, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    (target.elements.len() as u32).hash(h);
    for elem in &target.elements {
        match elem {
            Some(td) => hash_assignment_target_maybe_default(td, h, include_binding_names),
            // 省略位与有值位须区分：位置敏感。
            None => 0u8.hash(h),
        }
    }
    if let Some(rest) = &target.rest {
        1u8.hash(h);
        hash_assignment_target(&rest.target, h, include_binding_names);
    }
}

/// 哈希对象赋值目标：属性逐个哈希（键与绑定），rest 有无以哨兵区分。
pub(super) fn hash_object_assignment_target(
    target: &ObjectAssignmentTarget, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    (target.properties.len() as u32).hash(h);
    for prop in &target.properties {
        hash_assignment_target_property(prop, h, include_binding_names);
    }
    if let Some(rest) = &target.rest {
        1u8.hash(h);
        hash_assignment_target(&rest.target, h, include_binding_names);
    }
}

/// 哈希 `AssignmentTargetMaybeDefault`：带默认值形态计绑定递归加默认值表达式，
/// 其余变体（继承自 `AssignmentTarget`）转全量分派。
fn hash_assignment_target_maybe_default(
    target: &AssignmentTargetMaybeDefault, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    match target {
        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
            hash_assignment_target(&d.binding, h, include_binding_names);
            // 默认值表达式是字节码依赖（发射时求值入池），须计入。
            expression::hash_expression(&d.init, h, include_binding_names);
        }
        other => {
            if let Some(t) = other.as_assignment_target() {
                hash_assignment_target(t, h, include_binding_names);
            }
        }
    }
}

/// 哈希对象赋值目标属性：标识符形态计绑定名与默认值（若有），
/// 属性形态计键与绑定递归。
fn hash_assignment_target_property(
    prop: &AssignmentTargetProperty, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    match prop {
        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
            id.binding.name.as_str().hash(h);
            if let Some(init) = &id.init {
                expression::hash_expression(init, h, include_binding_names);
            }
        }
        AssignmentTargetProperty::AssignmentTargetPropertyProperty(pp) => {
            property::hash_property_key(&pp.name, h, include_binding_names);
            hash_assignment_target_maybe_default(&pp.binding, h, include_binding_names);
        }
    }
}
