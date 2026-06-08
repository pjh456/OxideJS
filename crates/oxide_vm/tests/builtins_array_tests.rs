use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn array_push_adds_element() {
    let result = eval("var a = [1,2]; a.push(3); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(obj.get_prop_at(2), oxide_types::value::JsValue::int(3));
}

#[test]
fn array_push_returns_length() {
    let result = eval("[1,2,3].push(4)").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn array_pop_returns_last() {
    let result = eval("[1,2,3].pop()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_pop_empty_returns_undefined() {
    let result = eval("[].pop()").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn array_pop_reduces_length() {
    let result = eval("var a = [1,2,3]; a.pop(); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn array_slice_returns_subarray() {
    let result = eval("[1,2,3,4].slice(1,3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0), oxide_types::value::JsValue::int(2));
    assert_eq!(obj.get_prop_at(1), oxide_types::value::JsValue::int(3));
}

#[test]
fn array_slice_no_args_copies() {
    let result = eval("[1,2,3].slice()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_concat_combines() {
    let result = eval("[1,2].concat([3,4])").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
    assert_eq!(obj.get_prop_at(3), oxide_types::value::JsValue::int(4));
}

#[test]
fn array_concat_non_array() {
    let result = eval("[1,2].concat(3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_join_with_dash() {
    let result = eval("['a','b','c'].join('-')").unwrap();
    assert!(result.is_string());
}

#[test]
fn array_index_of_found() {
    let result = eval("[10,20,30].indexOf(20)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn array_index_of_not_found() {
    let result = eval("[1,2,3].indexOf(99)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn array_includes_true() {
    let result = eval("[1,2,3].includes(2)").unwrap();
    assert_eq!(result.as_bool(), true);
}

#[test]
fn array_includes_false() {
    let result = eval("[1,2,3].includes(99)").unwrap();
    assert_eq!(result.as_bool(), false);
}

#[test]
fn array_reverse_mutates() {
    let result = eval("var a = [1,2,3]; a.reverse(); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.get_prop_at(0), oxide_types::value::JsValue::int(3));
    assert_eq!(obj.get_prop_at(1), oxide_types::value::JsValue::int(2));
    assert_eq!(obj.get_prop_at(2), oxide_types::value::JsValue::int(1));
}

#[test]
fn array_splice_remove() {
    let result = eval("[1,2,3,4].splice(1,2)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0), oxide_types::value::JsValue::int(2));
    assert_eq!(obj.get_prop_at(1), oxide_types::value::JsValue::int(3));
}

#[test]
fn array_splice_insert() {
    let result = eval("var a = [1,4]; a.splice(1,0,2,3); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_flat_noop() {
    let result = eval("[1,2,3].flat()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_flat_one_level() {
    let result = eval("[1,[2,3],4].flat()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_literal_creates_array() {
    let result = eval("[1,2,3]").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
}

#[test]
fn array_empty_literal() {
    let result = eval("[]").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn array_shift_removes_first() {
    let result = eval("[1,2,3].shift()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn array_unshift_adds_front() {
    let result = eval("[3].unshift(1,2)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_fill_replaces() {
    let result = eval("[1,2,3,4].fill(0,1,3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_at_positive() {
    let result = eval("[10,20,30].at(1)").unwrap();
    assert_eq!(result.as_int(), 20);
}

#[test]
fn array_at_negative() {
    let result = eval("[10,20,30].at(-1)").unwrap();
    assert_eq!(result.as_int(), 30);
}

#[test]
fn array_last_index_of_found() {
    let result = eval("[1,2,3,2].lastIndexOf(2)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_last_index_of_not_found() {
    let result = eval("[1,2,3].lastIndexOf(99)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn array_sort_default() {
    let result = eval("[3,1,2].sort()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_copy_within_copies() {
    let result = eval("[1,2,3,4,5].copyWithin(0,3)").unwrap();
    assert!(result.is_object());
}

#[test]
fn array_fill_single() {
    let result = eval("[1,2,3].fill(0)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.get_prop_at(0).as_int(), 0);
}
