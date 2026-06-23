use super::*;

pub(super) fn hash_class_element(element: &ClassElement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::ClassElement, element, h, {
        ClassElement::MethodDefinition(method) => {
            method.r#static.hash(h);
            method.computed.hash(h);
            std::mem::discriminant(&method.kind).hash(h);
            property::hash_property_key(&method.key, h, include_binding_names);
            if method.computed {
                property::hash_property_key(&method.key, h, include_binding_names);
            }
            (method.value.params.items.len() as u32).hash(h);
            if include_binding_names {
                for param in &method.value.params.items {
                    hash_binding_pattern(&param.pattern, h);
                }
            }
            if let Some(body) = &method.value.body {
                for stmt in &body.statements {
                    statement::hash_statement(stmt, h, include_binding_names);
                }
            }
        }
        ClassElement::PropertyDefinition(prop) => {
            prop.r#static.hash(h);
            prop.computed.hash(h);
            property::hash_property_key(&prop.key, h, include_binding_names);
            if let Some(value) = &prop.value {
                expression::hash_expression(value, h, include_binding_names);
            }
        }
        ClassElement::StaticBlock(block) => {
            for stmt in &block.body {
                statement::hash_statement(stmt, h, include_binding_names);
            }
        }
        _ => {}
    });
}
