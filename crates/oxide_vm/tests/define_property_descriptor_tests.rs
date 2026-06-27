use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<JsValue, String> {
    eval(source).map(|(_vm, v)| v)
}

fn assert_err_contains(result: Result<JsValue, String>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing '{}', got Ok", expected),
        Err(e) => assert!(e.contains(expected), "expected error containing '{}', got: {}", expected, e),
    }
}

// -- Partial writable update preserves existing value --
#[test]
fn define_property_partial_writable_preserves_value() {
    let r = eval_str("var obj={}; obj.x=42; Object.defineProperty(obj,'x',{writable:false}); obj.x").unwrap();
    assert!(r.is_int() && r.as_int() == 42, "value should be preserved at 42, got {:?}", r);
}

// -- Mixed descriptor (value+get) throws TypeError --
#[test]
fn define_property_mixed_value_get_throws() {
    let result = eval_str("var obj={}; Object.defineProperty(obj,'x',{value:1,get:function(){}});");
    assert_err_contains(result, "mix");
}

// -- Mixed descriptor (writable+set) throws TypeError --
#[test]
fn define_property_mixed_writable_set_throws() {
    let result = eval_str("var obj={}; Object.defineProperty(obj,'x',{writable:true,set:function(){}});");
    assert_err_contains(result, "mix");
}

// -- Partial enumerable-only update preserves value --
#[test]
fn define_property_partial_enumerable_only_preserves_value() {
    let r = eval_str("var obj={}; obj.x=42; Object.defineProperty(obj,'x',{enumerable:true}); obj.x").unwrap();
    assert!(r.is_int() && r.as_int() == 42, "value should be preserved at 42, got {:?}", r);
}

// -- getOwnPropertyDescriptor returns data fields (no get/set) --
#[test]
fn get_own_property_descriptor_data_fields() {
    let r = eval_str(
        "var obj={x:1}; var d=Object.getOwnPropertyDescriptor(obj,'x'); [d.value,d.writable,d.enumerable,d.configurable,d.get,d.set]",
    )
    .unwrap();
    let obj = unsafe { &*r.as_js_object_ptr() };
    let v = obj.get_prop_at(0);
    assert!(v.is_int() && v.as_int() == 1, "value should be 1, got {:?}", v);
    assert!(obj.get_prop_at(1).is_bool() && obj.get_prop_at(1).as_bool(), "writable should be true");
    assert!(obj.get_prop_at(2).is_bool() && obj.get_prop_at(2).as_bool(), "enumerable should be true");
    assert!(
        obj.get_prop_at(3).is_bool() && obj.get_prop_at(3).as_bool(),
        "configurable should be true"
    );
    assert!(obj.get_prop_at(4).is_undefined(), "get should be undefined");
    assert!(obj.get_prop_at(5).is_undefined(), "set should be undefined");
}

// -- getOwnPropertyDescriptor returns accessor fields (no value/writable) --
#[test]
fn get_own_property_descriptor_accessor_fields() {
    let r = eval_str(
        "var obj={}; Object.defineProperty(obj,'x',{get:function(){return 1;},enumerable:true,configurable:true}); var d=Object.getOwnPropertyDescriptor(obj,'x'); [d.get!==undefined,d.set===undefined,d.enumerable,d.configurable,d.value===undefined,d.writable===undefined]",
    )
    .unwrap();
    let obj = unsafe { &*r.as_js_object_ptr() };
    assert!(obj.get_prop_at(0).is_bool() && obj.get_prop_at(0).as_bool(), "get should be defined");
    assert!(obj.get_prop_at(1).is_bool() && obj.get_prop_at(1).as_bool(), "set should be undefined");
    assert!(obj.get_prop_at(2).is_bool() && obj.get_prop_at(2).as_bool(), "enumerable should be true");
    assert!(
        obj.get_prop_at(3).is_bool() && obj.get_prop_at(3).as_bool(),
        "configurable should be true"
    );
    assert!(obj.get_prop_at(4).is_bool() && obj.get_prop_at(4).as_bool(), "value should be undefined");
    assert!(
        obj.get_prop_at(5).is_bool() && obj.get_prop_at(5).as_bool(),
        "writable should be undefined"
    );
}

// -- New property without value or accessor defaults to data descriptor (ES spec behavior) --
#[test]
fn define_property_new_attributes_only_defaults_value_undefined() {
    let r = eval_str("var obj={}; Object.defineProperty(obj,'x',{enumerable:true}); obj.x").unwrap();
    assert!(r.is_undefined(), "new prop with only enumerable should default value to undefined, got {:?}", r);
}

// -- Partial configurable on existing property preserves value --
#[test]
fn define_property_partial_configurable_preserves_value() {
    let r = eval_str("var obj={}; obj.x=1; Object.defineProperty(obj,'x',{configurable:false}); [obj.x]").unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(arr.get_prop_at(0).is_int() && arr.get_prop_at(0).as_int() == 1, "value should be 1");
}
