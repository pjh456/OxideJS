use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

#[test]
fn eval_object_is_object() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "Object").unwrap();
    let module = Compiler::new().compile(&program).unwrap();
    let mut vm = Vm::new();
    let result = vm.run(&module).unwrap();
    assert!(
        result.is_object(),
        "Object should be an object, got {:?}",
        result
    );
}

#[test]
fn verify_global_object_props() {
    let vm = Vm::new();
    let global = vm.kernel().global_object();

    let obj_si = vm.kernel().string_forge().intern("Object").0;
    let mut shape_id = global.shape_id();
    let mut found = false;
    while shape_id != oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
        if let Some(shape) = vm.kernel().shape_forge().get_shape(shape_id) {
            if shape.property_name == obj_si {
                found = true;
                let val = global.get_prop(shape.property_offset);
                assert!(
                    val.is_object(),
                    "Object prop should be object, got {:?}",
                    val
                );
                break;
            }
            shape_id = shape
                .parent
                .unwrap_or(oxide_kernel::shape_forge::EMPTY_SHAPE_ID);
        } else {
            break;
        }
    }
    assert!(found, "Object property not found on global object");
}
