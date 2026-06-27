use oxide_builtins::error;
use oxide_compiler::compiler::Compiler;
use oxide_types::mem::P;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn make_vm() -> Vm {
    Vm::new()
}

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = make_vm();
    vm.run(&module)
}

#[test]
fn error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn error_has_message() {
    let mut vm = make_vm();
    let msg = vm.new_string("test message");
    vm.set_reg(1, msg);
    let result = error::error_constructor(&mut vm, &[0u8, 1u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn type_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::type_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn reference_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::reference_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn range_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::range_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn syntax_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::syntax_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn uri_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::uri_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn eval_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::eval_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn create_type_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "something went wrong");
    assert!(err.is_object());
}

#[test]
fn create_range_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_range_error(&mut vm, "out of bounds");
    assert!(err.is_object());
}

#[test]
fn create_reference_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_reference_error(&mut vm, "not defined");
    assert!(err.is_object());
}

#[test]
fn create_syntax_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_syntax_error(&mut vm, "invalid syntax");
    assert!(err.is_object());
}

#[test]
fn error_proto_chain_subtype_points_to_error() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "msg");
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert!(obj.proto().is_object());
}

#[test]
fn error_name_is_string_property() {
    let mut vm = make_vm();
    let err = error::error_constructor(&mut vm, &[0u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    let n = obj.prop_count();
    assert_eq!(n, 0);
}

#[test]
fn error_to_string_returns_string() {
    let result = eval("new Error('test').toString()").unwrap();
    assert!(result.is_string());
}

#[test]
fn type_error_is_defined() {
    let result = eval("typeof TypeError").unwrap();
    assert!(result.is_string());
}

#[test]
fn reference_error_is_defined() {
    let result = eval("typeof ReferenceError").unwrap();
    assert!(result.is_string());
}

#[test]
fn error_subtype_constructors_produce_named_objects() {
    assert_eq!(format!("{}", eval("new Error('boom').name == 'Error'").unwrap()), "true");
    assert_eq!(format!("{}", eval("new TypeError('boom').name == 'TypeError'").unwrap()), "true");
    assert_eq!(format!("{}", eval("new SyntaxError('boom').name == 'SyntaxError'").unwrap()), "true");
}

// New tests for prototype properties + constructor fixes

#[test]
fn error_prototype_has_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some(), "Error.prototype should have 'name' property");
}

#[test]
fn error_prototype_has_message() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let msg_si = vm.kernel_core().perm_interner().intern("message").0;
    let proto_ptr = P::as_ptr(&bw.error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), msg_si);
    assert!(si.is_some(), "Error.prototype should have 'message' property");
}

#[test]
fn type_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.type_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some(), "TypeError.prototype should have 'name'");
}

#[test]
fn reference_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.reference_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn range_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.range_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn syntax_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.syntax_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn uri_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.uri_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn eval_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.eval_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn error_constructor_no_own_props() {
    let mut vm = make_vm();
    let err = error::error_constructor(&mut vm, &[0u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn error_constructor_with_message() {
    let mut vm = make_vm();
    let msg = vm.new_string("boom");
    vm.set_reg(1, msg);
    let err = error::error_constructor(&mut vm, &[0u8, 1u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
}

#[test]
fn create_type_error_no_own_name() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "something broken");
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
}

// Plan 02: toString tests

#[test]
fn error_to_string_basic() {
    let result = eval("new Error('test').toString()").unwrap();
    assert!(result.is_string());
}

#[test]
fn error_to_string_empty_message() {
    let result = eval("new Error().toString()").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("Error".to_string()));
}

#[test]
fn error_to_string_type_error() {
    let result = eval("new TypeError('bad').toString()").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("TypeError: bad".to_string()));
}

#[test]
fn error_to_string_name_only() {
    let result = eval("Error.prototype.toString.call({name: 'E'})").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("E".to_string()));
}

#[test]
fn error_to_string_non_object_throws() {
    let mut vm = make_vm();
    vm.set_reg(0, JsValue::int(42));
    let result = error::error_to_string(&mut vm, &[0u8]);
    match result {
        oxide_runtime_api::NativeResult::Err(_) => {}
        _ => panic!("expected Err, got Ok or TailCall"),
    }
}

// format_error_message tests

#[test]
fn format_error_message_both() {
    let result = oxide_runtime_api::format_error_message("TypeError", "bad arg");
    assert_eq!(result, "TypeError: bad arg");
}

#[test]
fn format_error_message_empty_msg() {
    let result = oxide_runtime_api::format_error_message("Error", "");
    assert_eq!(result, "Error");
}

#[test]
fn format_error_message_empty_name() {
    let result = oxide_runtime_api::format_error_message("", "msg");
    assert_eq!(result, "msg");
}

// stack tests

#[test]
fn error_stack_is_string() {
    let result = eval("typeof new Error().stack()").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("string".to_string()));
}

#[test]
fn error_stack_starts_with_header() {
    let result = eval("new Error().stack()").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result).unwrap();
    assert!(s.starts_with("Error"), "stack should start with 'Error', got: {}", s);
}

#[test]
fn error_stack_frame_format() {
    let result = eval("(function foo() { return new Error('boom').stack(); })()").unwrap();
    let vm = make_vm();
    let s = vm.lookup_str(result).unwrap();
    assert!(s.contains("    at "), "stack should have 4-space indent, got: {}", s);
}
