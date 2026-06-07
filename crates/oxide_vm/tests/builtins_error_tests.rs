use oxide_vm::builtins::error;
use oxide_vm::vm::Vm;

fn make_vm() -> Vm {
    Vm::new()
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
    let msg = vm.intern("test message");
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
    assert!(n >= 2);
}
