//! 赋值目标（SimpleAssignmentTarget）的结构哈希。

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
