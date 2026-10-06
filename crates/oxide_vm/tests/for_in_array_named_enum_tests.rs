use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// 数组命名属性可枚举性判定与形状槽位、存储下标两口径对齐：for-in 枚举
// 命名属性时按元素区偏移取元数据，非空数组不可枚举命名属性被滤出、
// 元素不可枚举不连带误滤命名属性，与 Object.keys 同形。

#[test]
fn for_in_array_non_enum_named_property_filtered() {
    // 非空数组不可枚举命名属性不进入 for-in 枚举：
    // 单元素数组加不可枚举命名属性后仅枚举下标键。
    assert_eq!(
        eval(
            "var g = [1]; \
             Object.defineProperty(g, 'x', { value: 9, enumerable: false }); \
             var ka = []; for (var k in g) ka.push(k); \
             ka.length === 1 && ka[0] === '0'"
        ),
        "true",
        "non-enumerable named property on a non-empty array is filtered out of for-in"
    );
}

#[test]
fn for_in_array_multi_element_non_enum_named_filtered() {
    // 多元素数组可枚举命名属性保留、不可枚举命名属性滤出：
    // 下标键升序加可枚举命名属性，无不可枚举命名属性。
    assert_eq!(
        eval(
            "var h = [1, 2, 3]; h.p = 1; \
             Object.defineProperty(h, 'q', { value: 2, enumerable: false }); \
             var kb = []; for (var k in h) kb.push(k); \
             kb.length === 4 && kb[0] === '0' && kb[1] === '1' && kb[2] === '2' && kb[3] === 'p'"
        ),
        "true",
        "enumerable named property kept, non-enumerable one filtered on multi-element array"
    );
}

#[test]
fn for_in_array_non_enum_element_does_not_filter_named() {
    // 元素不可枚举不连带误滤命名属性：for-in 与 Object.keys 同形，
    // 两者都只含可枚举命名属性。
    assert_eq!(
        eval(
            "var n = [1]; \
             Object.defineProperty(n, '0', { value: 1, enumerable: false }); \
             n.named = 5; \
             var kc = []; for (var k in n) kc.push(k); \
             var keys = Object.keys(n); \
             kc.length === 1 && kc[0] === 'named' && keys.length === 1 && keys[0] === 'named'"
        ),
        "true",
        "non-enumerable array element does not drag the enumerable named property out of for-in"
    );
}

#[test]
fn for_in_array_accessor_presence_unchanged() {
    // accessor 命名属性存在性回归：Object.hasOwn / in / hasOwnProperty
    // 三判全真，形状槽命中即在场。
    assert_eq!(
        eval(
            "var a = [1]; \
             Object.defineProperty(a, 'ctor2', { get: function() { return 'g'; } }); \
             Object.hasOwn(a, 'ctor2') && ('ctor2' in a) && a.hasOwnProperty('ctor2')"
        ),
        "true",
        "accessor named property on array is present for hasOwn, in, hasOwnProperty"
    );
}

#[test]
fn for_in_array_accessor_getter_read_and_keys() {
    // accessor 命名属性读值与键表回归：getter 触发读值，
    // Object.keys 只含下标键不含 accessor 命名属性。
    assert_eq!(
        eval(
            "var a = [1]; \
             Object.defineProperty(a, 'ctor2', { get: function() { return 'g'; } }); \
             a.ctor2 === 'g' && Object.keys(a).length === 1 && Object.keys(a)[0] === '0'"
        ),
        "true",
        "accessor getter fires on read and Object.keys lists only the index key"
    );
}

#[test]
fn for_in_empty_array_accessor_and_data_zero_drift() {
    // 空数组 for-in 零扰动：空数组元素区偏移为零，
    // accessor 与数据命名属性按插入序全枚举。
    assert_eq!(
        eval(
            "var z = []; \
             Object.defineProperty(z, 'a', { get: function() { return 1; }, enumerable: true }); \
             z.b = 2; \
             var kz = []; for (var k in z) kz.push(k); \
             kz.length === 2 && kz[0] === 'a' && kz[1] === 'b'"
        ),
        "true",
        "empty array for-in enumerates accessor and data named properties in insertion order"
    );
}

#[test]
fn for_in_array_all_elements_enumerable_zero_drift() {
    // 全元素可枚举数组 for-in 零扰动：仅下标键升序枚举。
    assert_eq!(
        eval(
            "var t = [1, 2]; \
             var kt = []; for (var k in t) kt.push(k); \
             kt.length === 2 && kt[0] === '0' && kt[1] === '1'"
        ),
        "true",
        "plain array for-in enumerates index keys in ascending order"
    );
}

#[test]
fn for_in_ordinary_object_zero_drift() {
    // 普通对象 for-in 零漂移：命名属性按插入序全枚举，
    // 数组偏移分支不触及普通对象路径。
    assert_eq!(
        eval(
            "var o = { a: 1, b: 2 }; \
             var ko = []; for (var k in o) ko.push(k); \
             ko.length === 2 && ko[0] === 'a' && ko[1] === 'b'"
        ),
        "true",
        "ordinary object for-in keeps insertion order"
    );
}

#[test]
fn for_in_array_element_accessor_arm_untouched() {
    // 元素区 accessor 定义后读值、存在性、描述符三面不变：
    // 下标 0 的 accessor 触发 getter，hasOwn 判在，描述符 get 在场。
    assert_eq!(
        eval(
            "var j = [1, 2]; \
             Object.defineProperty(j, 0, { get: function() { return 'e'; } }); \
             j[0] === 'e' && Object.hasOwn(j, 0) && Object.getOwnPropertyDescriptor(j, 0).get !== undefined"
        ),
        "true",
        "element-area accessor keeps working: getter fires, hasOwn true, descriptor get present"
    );
}
