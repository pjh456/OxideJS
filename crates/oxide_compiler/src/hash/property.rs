//! 对象属性键/属性类型（PropertyKey/ObjectPropertyKind）的结构哈希。

use super::*;

pub(super) fn hash_property_key(key: &PropertyKey, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::PropertyKey, key, h, {
        PropertyKey::StaticIdentifier(ident) => {
            ident.name.as_str().hash(h);
        }
        PropertyKey::Identifier(ident) => {
            ident.name.as_str().hash(h);
        }
        PropertyKey::StringLiteral(s) => {
            s.value.hash(h);
        }
        PropertyKey::NumericLiteral(n) => {
            n.value.to_bits().hash(h);
        }
        PropertyKey::PrivateIdentifier(pi) => {
            pi.name.as_str().hash(h);
        }
        _ => {
            expression::hash_expression(key.to_expression(), h, include_binding_names);
        }
    });
}

pub(super) fn hash_object_property_kind(
    prop: &ObjectPropertyKind, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    hash_match!(HashDomain::ObjectPropertyKind, prop, h, {
        ObjectPropertyKind::ObjectProperty(p) => {
            std::mem::discriminant(&p.kind).hash(h);
            p.method.hash(h);
            p.computed.hash(h);
            hash_property_key(&p.key, h, include_binding_names);
            expression::hash_expression(&p.value, h, include_binding_names);
        }
        ObjectPropertyKind::SpreadProperty(spread) => {
            // spread 的源表达式计入哈希：`{...a}` 与 `{...b}` 结构不同，避免缓存碰撞。
            expression::hash_expression(&spread.argument, h, include_binding_names);
        }
    });
}
