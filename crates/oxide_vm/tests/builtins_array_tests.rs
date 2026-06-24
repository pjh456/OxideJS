use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::builtins::array::{array_constructor, array_push};
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
    Ok((vm, result))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

fn assert_num_eq(val: JsValue, expected: f64) {
    let actual = if val.is_int() { val.as_int() as f64 } else { val.as_double() };
    assert_eq!(actual, expected);
}

#[test]
fn array_push_adds_element() {
    let (_vm, result) = eval("var a = [1,2]; a.push(3); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(obj.get_prop_at(2), JsValue::int(3));
}

#[test]
fn array_push_returns_length() {
    let (_vm, result) = eval("[1,2,3].push(4)").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn array_push_returns_true_length_beyond_31() {
    let mut vm = Vm::new();
    vm.set_reg(1, oxide_types::value::JsValue::int(31));
    let array = array_constructor(&mut vm, &[0, 1]).unwrap();
    vm.set_reg(0, array);
    vm.set_reg(1, oxide_types::value::JsValue::int(99));
    let result = array_push(&mut vm, &[0, 1]).unwrap();
    assert_eq!(result.as_int(), 32);
}

#[test]
fn array_length_tracks_more_than_31_elements() {
    let (_vm, result) = eval("var a=[]; for (var i=0;i<40;i=i+1) a.push(i); a.length").unwrap();
    assert_eq!(result.as_int(), 40);
}

#[test]
fn array_push_preserves_slots_beyond_255() {
    let (_vm, result) = eval("var a=[]; for (var i=0;i<257;i=i+1) a.push(i); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 257);
    let tail = obj.get_prop_at(256);
    assert_eq!(tail.as_double(), 256.0);
}

#[test]
fn array_pop_returns_last() {
    let (_vm, result) = eval("[1,2,3].pop()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_pop_empty_returns_undefined() {
    let (_vm, result) = eval("[].pop()").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn array_pop_reduces_length() {
    let (_vm, result) = eval("var a = [1,2,3]; a.pop(); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn array_slice_returns_subarray() {
    let (_vm, result) = eval("[1,2,3,4].slice(1,3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0), JsValue::int(2));
    assert_eq!(obj.get_prop_at(1), JsValue::int(3));
}

#[test]
fn array_slice_no_args_copies() {
    let (_vm, result) = eval("[1,2,3].slice()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_concat_combines() {
    let (_vm, result) = eval("[1,2].concat([3,4])").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
    assert_eq!(obj.get_prop_at(3), JsValue::int(4));
}

#[test]
fn array_concat_non_array() {
    let (_vm, result) = eval("[1,2].concat(3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_join_with_dash() {
    let (_vm, result) = eval("['a','b','c'].join('-')").unwrap();
    assert!(result.is_string());
}

#[test]
fn array_index_of_found() {
    let (_vm, result) = eval("[10,20,30].indexOf(20)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn array_index_of_not_found() {
    let (_vm, result) = eval("[1,2,3].indexOf(99)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn array_includes_true() {
    let (_vm, result) = eval("[1,2,3].includes(2)").unwrap();
    assert_eq!(result.as_bool(), true);
}

#[test]
fn array_includes_false() {
    let (_vm, result) = eval("[1,2,3].includes(99)").unwrap();
    assert_eq!(result.as_bool(), false);
}

#[test]
fn array_reverse_mutates() {
    let (_vm, result) = eval("var a = [1,2,3]; a.reverse(); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.get_prop_at(0), JsValue::int(3));
    assert_eq!(obj.get_prop_at(1), JsValue::int(2));
    assert_eq!(obj.get_prop_at(2), JsValue::int(1));
}

#[test]
fn array_splice_remove() {
    let (_vm, result) = eval("[1,2,3,4].splice(1,2)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0), JsValue::int(2));
    assert_eq!(obj.get_prop_at(1), JsValue::int(3));
}

#[test]
fn array_splice_insert() {
    let (_vm, result) = eval("var a = [1,4]; a.splice(1,0,2,3); a").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_flat_noop() {
    let (_vm, result) = eval("[1,2,3].flat()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_flat_one_level() {
    let (_vm, result) = eval("[1,[2,3],4].flat()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_literal_creates_array() {
    let (_vm, result) = eval("[1,2,3]").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
}

#[test]
fn array_empty_literal() {
    let (_vm, result) = eval("[]").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn array_is_array_static_method() {
    let (_vm, result) = eval("Array.isArray([])").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let (_vm, result) = eval("Array.isArray({})").unwrap();
    assert_eq!(result, JsValue::bool(false));
}

#[test]
fn array_shift_removes_first() {
    let (_vm, result) = eval("[1,2,3].shift()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn array_unshift_adds_front() {
    let (_vm, result) = eval("[3].unshift(1,2)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_fill_replaces() {
    let (_vm, result) = eval("[1,2,3,4].fill(0,1,3)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 4);
}

#[test]
fn array_at_positive() {
    let (_vm, result) = eval("[10,20,30].at(1)").unwrap();
    assert_eq!(result.as_int(), 20);
}

#[test]
fn array_at_negative() {
    let (_vm, result) = eval("[10,20,30].at(-1)").unwrap();
    assert_eq!(result.as_int(), 30);
}

#[test]
fn array_last_index_of_found() {
    let (_vm, result) = eval("[1,2,3,2].lastIndexOf(2)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_last_index_of_not_found() {
    let (_vm, result) = eval("[1,2,3].lastIndexOf(99)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn array_sort_default() {
    let (_vm, result) = eval("[3,1,2].sort()").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn array_map_filter_reduce_accept_bytecode_callbacks() {
    let (vm, result) = eval("[1,2,3].map(function(x){return x+1}).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "2,3,4");

    let (vm, result) = eval("[1,2,3].map(x => x * 2).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "2,4,6");

    let (vm, result) = eval("[1,2,3,4].filter(x => x % 2 === 0).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "2,4");

    let (_vm, result) = eval("[1,2,3].reduce((a,b)=>a+b,0)").unwrap();
    assert_num_eq(result, 6.0);
}

#[test]
fn array_iteration_predicates_accept_bytecode_callbacks() {
    let (_vm, result) = eval("[1,2,3].forEach(function(x){return x+1})").unwrap();
    assert!(result.is_undefined());

    let (_vm, result) = eval("[1,2,3].find(x => x > 1)").unwrap();
    assert_num_eq(result, 2.0);

    let (_vm, result) = eval("[1,2,3].some(x => x === 2)").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let (_vm, result) = eval("[1,2,3].every(function(x){return x > 0})").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn array_callback_exceptions_propagate() {
    let (vm, result) = eval("try{[1].forEach(()=>{throw new TypeError('boom')})}catch(e){e.message}").unwrap();
    assert_eq!(to_str(&vm, result), "boom");

    let (vm, result) = eval("try{[1].map(()=>{throw new Error('x')})}catch(e){e.message}").unwrap();
    assert_eq!(to_str(&vm, result), "x");

    let (vm, result) = eval("try{[1].filter(()=>{throw new Error('f')})}catch(e){e.message}").unwrap();
    assert_eq!(to_str(&vm, result), "f");
}

#[test]
fn array_sort_uses_user_comparator() {
    let (vm, result) = eval("[3,1,2].sort((a,b)=>a-b).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "1,2,3");

    let (vm, result) = eval("[3,1,2].sort((a,b)=>b-a).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "3,2,1");

    let (vm, result) = eval("[10,9,1].sort().join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "1,10,9");
}

#[test]
fn array_sort_propagates_comparator_exception() {
    let (vm, result) = eval("try{[1,2,3].sort(()=>{throw new Error('cmp')})}catch(e){e.message}").unwrap();
    assert_eq!(to_str(&vm, result), "cmp");
}

#[test]
fn array_callback_type_errors_are_not_sentinels() {
    let (vm, result) = eval("try { [1].map(null) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");

    let (vm, result) = eval("try { [1].filter(undefined) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");

    let (vm, result) = eval("try { [1].findIndex(0) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn array_reduce_and_sort_invalid_usage_throw_type_error() {
    let (vm, result) = eval("try { [].reduce(function(a, b) { return a + b; }) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");

    let (vm, result) = eval("try { [].reduceRight(function(a, b) { return a + b; }) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");

    let (vm, result) = eval("try { [1].sort(null) } catch (e) { e.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");

    let (vm, result) = eval("[2,1].sort(undefined).join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "1,2");
}

#[test]
fn array_copy_within_copies() {
    let (_vm, result) = eval("[1,2,3,4,5].copyWithin(0,3)").unwrap();
    assert!(result.is_object());
}

#[test]
fn array_fill_single() {
    let (_vm, result) = eval("[1,2,3].fill(0)").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.get_prop_at(0).as_int(), 0);
}
