//! 数组 length 虚拟属性的 define 侧回归：`Object.defineProperty` / `Reflect.defineProperty`
//! 的截断、增长、可写位收窄、非法长度 RangeError 与访问器拒绝。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 求值并取字符串结果内容（字符串值走 lookup_str，非字符串走 Display）。
fn eval_str(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse ok");
    let module = Compiler::new().compile(&program).expect("compile ok");
    let mut vm = Vm::new();
    let val = vm.run(&Arc::new(module)).expect("run ok");
    if val.is_string() {
        vm.lookup_str(val).unwrap_or_default()
    } else {
        format!("{val}")
    }
}

#[test]
fn define_length_value_truncates_elements() {
    // value 收缩：元素区被截断，越界元素不可见。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{value:1}); ''+a.length+':'+a[2]+':'+(2 in a)"),
        "1:undefined:false"
    );
    // value 增长：新增槽为稀疏空洞。
    assert_eq!(
        eval_str("var a=[1]; Object.defineProperty(a,'length',{value:3}); ''+a.length+':'+(1 in a)+':'+(2 in a)"),
        "3:false:false"
    );
}

#[test]
fn define_length_value_writable_defaults_to_current() {
    // 只给 value：writable 缺省保持当前值 true。
    assert_eq!(
        eval_str("var a=[1,2]; Object.defineProperty(a,'length',{value:1}); ''+Object.getOwnPropertyDescriptor(a,'length').writable"),
        "true"
    );
}

#[test]
fn define_length_non_writable_blocks_sloppy_assignment() {
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); (function(){a.length=1;})(); ''+a.length"),
        "3"
    );
}

#[test]
fn define_length_non_writable_throws_strict_assignment() {
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); (function(){'use strict'; try{a.length=1;return 'no-throw';}catch(e){return e.name+':'+a.length;}})()"),
        "TypeError:3"
    );
}

#[test]
fn define_length_non_writable_same_value_succeeds() {
    assert_eq!(
        eval_str("var a=[1,2]; Object.defineProperty(a,'length',{writable:false}); Object.defineProperty(a,'length',{value:2}); ''+a.length"),
        "2"
    );
}

#[test]
fn define_length_invalid_values_throw_range_error() {
    let src = "var r=[]; [-1,NaN,4294967296,1.5].forEach(function(v){try{Object.defineProperty([],'length',{value:v});r.push('no-throw');}catch(e){r.push(e.name);}}); r.join(',')";
    assert_eq!(eval_str(src), "RangeError,RangeError,RangeError,RangeError");
    // 非法值检查早于 configurable 收窄校验。
    assert_eq!(
        eval_str("try{Object.defineProperty([],'length',{value:-1,configurable:true});'no-throw';}catch(e){e.name}"),
        "RangeError"
    );
}

#[test]
fn define_length_descriptor_validation_throws_type_error() {
    // configurable/enumerable 不得放宽。
    assert_eq!(
        eval_str("try{Object.defineProperty([],'length',{configurable:true});'no-throw';}catch(e){e.name}"),
        "TypeError"
    );
    // 非可写后不得再请求 writable。
    assert_eq!(
        eval_str("var a=[1]; Object.defineProperty(a,'length',{writable:false}); try{Object.defineProperty(a,'length',{writable:true});'no-throw';}catch(e){e.name}"),
        "TypeError"
    );
}

#[test]
fn define_length_accessor_descriptor_rejected() {
    assert_eq!(
        eval_str("var r=[]; try{Object.defineProperty([],'length',{get:function(){return 1;}});r.push('no-throw');}catch(e){r.push(e.name);} r.push(Reflect.defineProperty([],'length',{set:function(v){}})); r.join(',')"),
        "TypeError,false"
    );
}

#[test]
fn define_length_shrink_blocked_by_non_configurable_element() {
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'2',{configurable:false}); var r; try{Object.defineProperty(a,'length',{value:1});r='no-throw';}catch(e){r=e.name;} r+':'+a.length"),
        "TypeError:3"
    );
}

#[test]
fn define_length_configurable_true_error_leaves_elements() {
    // TypeError 后元素区不截断。
    assert_eq!(
        eval_str("var a=[1,2,3]; try{Object.defineProperty(a,'length',{value:1,configurable:true});}catch(e){} ''+a.length+':'+a[2]"),
        "3:3"
    );
}

#[test]
fn define_length_coerces_value_twice_before_validation() {
    // ToUint32 + ToNumber 各触发一次 valueOf；第二次强转的副作用收窄可写位后
    // 由描述符校验拒绝。
    assert_eq!(
        eval_str("var a=[1,2], n=0; var v={valueOf:function(){n++;return a.length;}}; Object.defineProperty(a,'length',{value:v}); ''+a.length+':'+n"),
        "2:2"
    );
    assert_eq!(
        eval_str("var a=[1,2,3], n=0; var v={valueOf:function(){n++;Object.defineProperty(a,'length',{writable:false});return a.length;}}; var r; try{Object.defineProperty(a,'length',{value:v,writable:true});r='no-throw';}catch(e){r=e.name;} r+':'+n"),
        "TypeError:2"
    );
}

#[test]
fn reflect_define_length_error_channel() {
    // 非法长度保留 RangeError kind（抛而非 false）；普通失败投影 false。
    assert_eq!(
        eval_str("var r=[]; try{Reflect.defineProperty([],'length',{value:-1});r.push('no-throw');}catch(e){r.push(e.name);} r.push(Reflect.defineProperty([],'length',{configurable:true})); r.join(',')"),
        "RangeError,false"
    );
}

#[test]
fn non_array_length_stays_ordinary_property() {
    assert_eq!(
        eval_str("var o={}; Object.defineProperty(o,'length',{value:1}); ''+o.length+':'+Object.getOwnPropertyDescriptor(o,'length').writable"),
        "1:false"
    );
}

#[test]
fn define_length_non_writable_reported_by_descriptor() {
    assert_eq!(
        eval_str("var a=[1]; Object.defineProperty(a,'length',{writable:false}); ''+Object.getOwnPropertyDescriptor(a,'length').writable+':'+Object.getOwnPropertyDescriptor(a,'length').value"),
        "false:1"
    );
}

#[test]
fn define_index_at_length_requires_writable_length() {
    // 索引达到当前 length 需增长 length，length 不可写则拒绝。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); var r; try{Object.defineProperty(a,3,{value:'x'});r='no-throw';}catch(e){r=e.name;} r+':'+a.length"),
        "TypeError:3"
    );
    // 索引小于 length 的已有元素重定义不受 length 可写性限制。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); Object.defineProperty(a,1,{value:9}); ''+a[1]"),
        "9"
    );
}
