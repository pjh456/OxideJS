//! 03.2 数组元素访问 dispatch 内联 fast path 语义测试：验证 fast path 命中
//! （密集数组 int 键界内直读写）与各回退场景（越界、hole、accessor、非数组、
//! 非对象 receiver）与 ordinary 属性路径行为一致。

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

fn eval_str(source: &str) -> Result<String, String> {
    let (_vm, v) = eval(source)?;
    if v.is_string() {
        Ok(oxide_runtime_api::to_string(v))
    } else {
        Ok(format!("{v}"))
    }
}

// 密集数组整数键界内读：命中 fast path 直读元素区。
#[test]
fn dense_index_read_hits_fast_path() {
    assert_eq!(eval_str("var a=[1,2,3]; a[1]").unwrap(), "2");
    assert_eq!(eval_str("var a=[10,20,30]; a[0]+a[2]").unwrap(), "40");
}

// 密集数组整数键界内写：命中 fast path 直写元素区，读回一致。
#[test]
fn dense_index_write_hits_fast_path() {
    assert_eq!(eval_str("var a=[1,2,3]; a[1]=9; a[1]").unwrap(), "9");
    assert_eq!(eval_str("var a=[1,2,3]; a[0]=5; a[0]").unwrap(), "5");
}

// 字符串规范数字键与整数键等价（`a["1"]` == `a[1]`，同一 fast path 键）。
#[test]
fn string_canonical_index_key_equivalent() {
    assert_eq!(eval_str("var a=[1,2,3]; a[\"1\"]").unwrap(), "2");
    assert_eq!(eval_str("var a=[1,2,3]; a[\"1\"]=7; a[1]").unwrap(), "7");
}

// 整值 double 键（`a[1.0]`）与整数键等价。
#[test]
fn integral_double_key_equivalent() {
    assert_eq!(eval_str("var a=[1,2,3]; a[1.0]").unwrap(), "2");
}

// 越界读不命中 fast path，落原型链（undefined / 原型上元素）。
#[test]
fn out_of_bounds_read_falls_to_proto_chain() {
    assert_eq!(eval_str("var a=[1,2,3]; a[5]").unwrap(), "undefined");
    assert_eq!(eval_str("var a=[1,2,3]; a.__proto__={5:\"x\"}; a[5]").unwrap(), "x");
    assert_eq!(eval_str("Array.prototype[9]=\"proto9\"; var a=[1,2,3]; a[9]").unwrap(), "proto9");
}

// 越界写不命中 fast path，走 CreateDataProperty + length 扩展。
#[test]
fn out_of_bounds_write_extends_length() {
    assert_eq!(eval_str("var a=[1,2,3]; a[5]=50; a.length").unwrap(), "6");
    assert_eq!(eval_str("var a=[1,2,3]; a[5]=50; a[5]").unwrap(), "50");
}

// delete 后的 hole：元素 meta 非空，不命中 fast path，读落原型链。
#[test]
fn deleted_hole_read_falls_to_proto() {
    assert_eq!(eval_str("var a=[1,2,3]; delete a[1]; a[1]").unwrap(), "undefined");
}

// defineProperty accessor：元素 meta 非空，不命中 fast path，getter/setter 仍触发。
#[test]
fn accessor_getter_and_setter_still_trigger() {
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,1,{get:function(){return 99;}}); a[1]").unwrap(),
        "99"
    );
    assert_eq!(
        eval_str(
            "var a=[1,2,3]; Object.defineProperty(a,1,{set:function(v){this._s=v;},get:function(){return this._s;}}); a[1]=7; a[1]"
        )
        .unwrap(),
        "7"
    );
}

// 非写属性（writable:false）写入：fast path 不命中，走完整协议抛 TypeError。
#[test]
fn non_writable_index_write_throws() {
    let r = eval_str(
        "var a=[1,2,3]; Object.defineProperty(a,0,{writable:false,value:1}); try { a[0]=9; 'no' } catch(e) { e instanceof TypeError ? 'throws' : 'wrong' }"
    )
    .unwrap();
    assert_eq!(r, "throws", "expected TypeError, got {r:?}");
}

// 非规范数字串（前导零）不映射整数键，走字符串键慢路径（不命中 fast path）。
#[test]
fn non_canonical_index_string_uses_slow_path() {
    assert_eq!(eval_str("var a=[1,2,3]; a[\"05\"]").unwrap(), "undefined");
    assert_eq!(eval_str("var a=[1,2,3]; a[\"05\"]=9; a.length").unwrap(), "3");
}

// 负索引/小数键不进入整数键区间，走字符串键慢路径。
#[test]
fn negative_and_fractional_keys_use_slow_path() {
    assert_eq!(eval_str("var a=[1,2,3]; a[-1]").unwrap(), "undefined");
    assert_eq!(eval_str("var a=[1,2,3]; a[1.5]").unwrap(), "undefined");
}

// 普通对象（非数组）整数键不受 fast path 影响，走普通属性路径。
#[test]
fn plain_object_int_key_unaffected() {
    assert_eq!(eval_str("var o={0:\"x\"}; o[0]").unwrap(), "x");
    assert_eq!(eval_str("var o={}; o[0]=\"y\"; o[0]").unwrap(), "y");
}

// 非对象 receiver（`"abc"[0]`）：is_object 快检先行，不 raise、返回 undefined。
#[test]
fn primitive_string_index_does_not_raise() {
    assert_eq!(eval_str("\"abc\"[0]").unwrap(), "undefined");
    assert_eq!(eval_str("var a=\"abc\"; a[0]").unwrap(), "undefined");
}

// push 构造的数组 fast path 读写与 length 一致。
#[test]
fn push_built_array_fast_path_consistent() {
    assert_eq!(eval_str("var a=[]; a.push(1); a.push(2); a.length").unwrap(), "2");
    assert_eq!(eval_str("var a=[]; a.push(1); a.push(2); a[0]+a[1]").unwrap(), "3");
}

// TypedArray 整数索引不命中数组 fast path（is_array 为假），走 TA 分支。
#[test]
fn typed_array_int_index_unaffected() {
    assert_eq!(eval_str("var t=new Int32Array([5,6,7]); t[1]").unwrap(), "6");
}

// fast path 写后 length 读一致（shadow length 同步不受影响）。
#[test]
fn fast_path_write_keeps_length_in_sync() {
    assert_eq!(eval_str("var a=[1,2,3]; a[0]=7; a.length").unwrap(), "3");
}

// 空数组 / 稀疏空洞（字面量 hole，无 meta）读 undefined，与慢路径一致。
#[test]
fn sparse_literal_hole_reads_undefined() {
    assert_eq!(eval_str("var a=[10,,30]; a[1]").unwrap(), "undefined");
}

// 越界写后读取原型链元素（fast path 不越界直写）。
#[test]
fn oob_write_then_proto_read() {
    assert_eq!(eval_str("Array.prototype[9]=\"proto9\"; var a=[1,2,3]; a[9]=90; a[9]").unwrap(), "90");
}
