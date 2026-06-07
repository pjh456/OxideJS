use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::StringForge;

pub struct ObjectMethods {
    pub keys: *const (),
    pub create: *const (),
    pub assign: *const (),
    pub define_property: *const (),
    pub get_own_property_descriptor: *const (),
}

pub struct ArrayMethods {
    pub push: *const (),
    pub pop: *const (),
    pub slice: *const (),
    pub splice: *const (),
    pub concat: *const (),
    pub join: *const (),
    pub index_of: *const (),
    pub includes: *const (),
    pub reverse: *const (),
    pub for_each: *const (),
    pub map: *const (),
    pub filter: *const (),
    pub reduce: *const (),
    pub find: *const (),
    pub some: *const (),
    pub every: *const (),
    pub flat: *const (),
    pub flat_map: *const (),
}

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

fn intern_label(string_forge: &StringForge, label: &str) -> u32 {
    string_forge.intern(label).0
}

fn make_pair(
    string_forge: &StringForge,
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
    pub fn new(string_forge: &StringForge, shape_forge: &ShapeForge) -> Self {
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

    pub fn bind_object_methods(
        &self,
        methods: &ObjectMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let ctor_ptr = P::as_ptr(&self.object_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };

        let _ = Self::bind_method(ctor, shape_forge, string_forge, "keys", methods.keys, 1);
        let _ = Self::bind_method(ctor, shape_forge, string_forge, "create", methods.create, 2);
        let _ = Self::bind_method(ctor, shape_forge, string_forge, "assign", methods.assign, 2);
        let _ = Self::bind_method(
            ctor,
            shape_forge,
            string_forge,
            "defineProperty",
            methods.define_property,
            3,
        );
        let _ = Self::bind_method(
            ctor,
            shape_forge,
            string_forge,
            "getOwnPropertyDescriptor",
            methods.get_own_property_descriptor,
            2,
        );
    }

    pub fn bind_array_methods(
        &self,
        methods: &ArrayMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let proto_ptr = P::as_ptr(&self.array_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };

        let _ = Self::bind_method(proto, shape_forge, string_forge, "push", methods.push, 1);
        let _ = Self::bind_method(proto, shape_forge, string_forge, "pop", methods.pop, 0);
        let _ = Self::bind_method(proto, shape_forge, string_forge, "slice", methods.slice, 2);
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "splice",
            methods.splice,
            2,
        );
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "concat",
            methods.concat,
            1,
        );
        let _ = Self::bind_method(proto, shape_forge, string_forge, "join", methods.join, 1);
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "indexOf",
            methods.index_of,
            1,
        );
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "includes",
            methods.includes,
            1,
        );
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "reverse",
            methods.reverse,
            0,
        );
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "forEach",
            methods.for_each,
            1,
        );
        let _ = Self::bind_method(proto, shape_forge, string_forge, "map", methods.map, 1);
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "filter",
            methods.filter,
            1,
        );
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "reduce",
            methods.reduce,
            1,
        );
        let _ = Self::bind_method(proto, shape_forge, string_forge, "find", methods.find, 1);
        let _ = Self::bind_method(proto, shape_forge, string_forge, "some", methods.some, 1);
        let _ = Self::bind_method(proto, shape_forge, string_forge, "every", methods.every, 1);
        let _ = Self::bind_method(proto, shape_forge, string_forge, "flat", methods.flat, 0);
        let _ = Self::bind_method(
            proto,
            shape_forge,
            string_forge,
            "flatMap",
            methods.flat_map,
            1,
        );
    }

    pub fn bind_method(
        proto: &mut JsObject,
        shape_forge: &ShapeForge,
        string_forge: &StringForge,
        method_name: &str,
        native_fn_ptr: *const (),
        arg_count: u8,
    ) -> Result<(), String> {
        let si = string_forge.intern(method_name).0;
        let mut wrapper = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        wrapper.set_function(true);
        wrapper.set_native_fn(Some(native_fn_ptr));
        wrapper.set_native_arg_count(arg_count);
        let wrapper_val = JsValue::from_js_object(Box::into_raw(wrapper));
        let new_offset = proto.prop_count();
        let new_shape = shape_forge.make_shape(proto.shape_id(), si);
        proto.set_shape_id(new_shape);
        proto.set_prop_count(new_offset + 1);
        proto.set_prop_expand_heap(new_offset, wrapper_val);
        proto.bump_generation();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
    use crate::string_forge::StringForge;

    fn make_world() -> BuiltinWorld {
        let sf = StringForge::new();
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
