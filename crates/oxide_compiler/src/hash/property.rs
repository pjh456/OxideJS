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
        ObjectPropertyKind::SpreadProperty(_) => {}
    });
}
