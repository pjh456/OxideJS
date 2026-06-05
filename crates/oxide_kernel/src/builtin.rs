use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::Interner;

pub struct BuiltinWorld {
    pub object_proto: P<JsObject>,
    pub array_proto: P<JsObject>,
    pub function_proto: P<JsObject>,
    pub string_proto: P<JsObject>,
    pub number_proto: P<JsObject>,
    pub boolean_proto: P<JsObject>,
    pub error_proto: P<JsObject>,
    pub symbol_proto: P<JsObject>,
    pub object_constructor: P<JsObject>,
    pub array_constructor: P<JsObject>,
    pub function_constructor: P<JsObject>,
    pub string_constructor: P<JsObject>,
    pub number_constructor: P<JsObject>,
    pub boolean_constructor: P<JsObject>,
    pub error_constructor: P<JsObject>,
    pub symbol_constructor: P<JsObject>,
}

fn intern_label(interner: &Interner, label: &str) -> u32 {
    interner.intern(label).0
}

fn make_pair(
    string_forge: &Interner,
    shape_forge: &ShapeForge,
    name: &str,
    si_prototype: u32,
    si_constructor: u32,
    si_name: u32,
) -> (P<JsObject>, P<JsObject>) {
    intern_label(string_forge, name);

    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

    let proto_shape = shape_forge.make_shape(EMPTY_SHAPE_ID, si_constructor);
    proto.set_shape_id(proto_shape);
    proto.set_prop_count(1);

    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.set_prop_count(2);
    ctor.set_function(true);

    proto.set_inline_prop(0, JsValue::undefined());
    ctor.set_inline_prop(0, JsValue::undefined());
    ctor.set_inline_prop(1, JsValue::undefined());

    (P::new(proto), P::new(ctor))
}

impl BuiltinWorld {
    pub fn new(string_forge: &Interner, shape_forge: &ShapeForge) -> Self {
        let si_prototype = intern_label(string_forge, "prototype");
        let si_constructor = intern_label(string_forge, "constructor");
        let si_name = intern_label(string_forge, "name");
        intern_label(string_forge, "length");
        intern_label(string_forge, "toString");
        intern_label(string_forge, "valueOf");

        let (object_proto, object_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Object",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (array_proto, array_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Array",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (function_proto, function_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Function",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (string_proto, string_constructor) = make_pair(
            string_forge,
            shape_forge,
            "String",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (number_proto, number_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Number",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (boolean_proto, boolean_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Boolean",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (error_proto, error_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Error",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (symbol_proto, symbol_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Symbol",
            si_prototype,
            si_constructor,
            si_name,
        );

        Self {
            object_proto,
            array_proto,
            function_proto,
            string_proto,
            number_proto,
            boolean_proto,
            error_proto,
            symbol_proto,
            object_constructor,
            array_constructor,
            function_constructor,
            string_constructor,
            number_constructor,
            boolean_constructor,
            error_constructor,
            symbol_constructor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
    use crate::string_forge::Interner;

    fn make_world() -> BuiltinWorld {
        let sf = Interner::new();
        let sh = ShapeForge::new();
        BuiltinWorld::new(&sf, &sh)
    }

    #[test]
    fn test_all_protos_valid() {
        let w = make_world();
        let protos = [
            &w.object_proto,
            &w.array_proto,
            &w.function_proto,
            &w.string_proto,
            &w.number_proto,
            &w.boolean_proto,
            &w.error_proto,
            &w.symbol_proto,
        ];
        for p in protos {
            assert!(
                p.shape_id() > EMPTY_SHAPE_ID,
                "proto should have a non-empty shape"
            );
        }
    }

    #[test]
    fn test_all_constructors_valid() {
        let w = make_world();
        assert!(w.object_constructor.is_function());
        assert!(w.array_constructor.is_function());
        assert!(w.function_constructor.is_function());
        assert!(w.string_constructor.is_function());
        assert!(w.number_constructor.is_function());
        assert!(w.boolean_constructor.is_function());
        assert!(w.error_constructor.is_function());
        assert!(w.symbol_constructor.is_function());
    }

    #[test]
    fn test_prototypes_are_not_functions() {
        let w = make_world();
        assert!(!w.object_proto.is_function());
        assert!(!w.array_proto.is_function());
        assert!(!w.function_proto.is_function());
        assert!(!w.string_proto.is_function());
        assert!(!w.number_proto.is_function());
        assert!(!w.boolean_proto.is_function());
        assert!(!w.error_proto.is_function());
        assert!(!w.symbol_proto.is_function());
    }

    #[test]
    fn test_protos_have_null_proto() {
        let w = make_world();
        assert!(w.object_proto.proto().is_null());
        assert!(w.array_proto.proto().is_null());
        assert!(w.function_proto.proto().is_null());
        assert!(w.string_proto.proto().is_null());
        assert!(w.number_proto.proto().is_null());
        assert!(w.boolean_proto.proto().is_null());
        assert!(w.error_proto.proto().is_null());
        assert!(w.symbol_proto.proto().is_null());
    }

    #[test]
    fn test_shapes_populated() {
        let w = make_world();
        assert!(
            w.object_constructor.shape_id() > EMPTY_SHAPE_ID,
            "constructor should have prototype + name shape"
        );
        assert!(
            w.object_proto.shape_id() > EMPTY_SHAPE_ID,
            "prototype should have constructor shape"
        );
    }
}
