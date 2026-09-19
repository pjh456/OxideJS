//! splice/reverse/copyWithin/at 四方法洞语义与原型链读语义引擎钉：
//! 源洞保洞/换洞/删洞、重叠反向复制、元素读走 Get。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval_value(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, result) = eval_value(source)?;
    vm.lookup_str(result)
        .ok_or_else(|| "completion value is not a string".to_string())
}

// splice 被删收集：源洞位在 removed 保洞，present 值原样落入。
#[test]
fn test_splice_removed_keeps_holes() {
    let out = eval_str(
        "(() => { const a = new Array(3); a[1] = 1; a[2] = 3; \
         const r = a.splice(0, 3); \
         return r.length + '|' + (0 in r) + '|' + r[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "3|false|1");
}

// splice 被删收集走 Get：洞位落原型链，访问器值落入 removed 且该位为 present。
#[test]
fn test_splice_removed_get_through_proto() {
    let out = eval_str(
        "(() => { Object.defineProperty(Array.prototype, 0, { get: () => 'G0' }); \
         try { const a = new Array(2); a[1] = 1; const r = a.splice(0, 2); \
         return r[0] + '|' + (0 in r) + '|' + r.length; } \
         finally { delete Array.prototype[0]; } })()",
    )
    .unwrap();
    assert_eq!(out, "G0|true|2");
}

// splice 扩容：洞洞源 removed 全洞；数组体洞位随搬移保留、插入位 present。
#[test]
fn test_splice_grow_keeps_array_holes() {
    let out = eval_str(
        "(() => { const a = new Array(3); const r = a.splice(0, 2, 'a'); \
         return (0 in r) + '|' + (1 in r) + '|' + r.length + '|' + \
         (0 in a) + '|' + (1 in a) + '|' + a.length; })()",
    )
    .unwrap();
    assert_eq!(out, "false|false|2|true|false|2");
}

// splice 中插：插入点之后的既有洞位随搬移落位，`3 in` 为 false。
#[test]
fn test_splice_insert_mid_hole_moves() {
    let out = eval_str(
        "(() => { const a = new Array(4); a[0] = 1; a[1] = 2; a[3] = 4; \
         a.splice(2, 0, 'x'); \
         return (3 in a) + '|' + (2 in a) + '|' + a[2] + '|' + a.length; })()",
    )
    .unwrap();
    assert_eq!(out, "false|true|x|5");
}

// reverse 洞对 present：值位互换、洞位互换，洞不物化。
#[test]
fn test_reverse_hole_and_value_swap() {
    let out = eval_str(
        "(() => { const a = new Array(3); a[0] = 1; a[2] = 3; a.reverse(); \
         return (1 in a) + '|' + a[0] + '|' + a[2]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|3|1");
}

// reverse 洞对洞：双缺失端不动，洞位互换后各归其位。
#[test]
fn test_reverse_hole_to_hole() {
    let out = eval_str(
        "(() => { const a = new Array(3); a[1] = 2; a[2] = 3; a.reverse(); \
         return (2 in a) + '|' + a[0] + '|' + a[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|3|2");
}

// reverse 原型链存在性：自身洞 + 原型 present 视同双存在，两端互写为 present。
#[test]
fn test_reverse_proto_present_both_sides() {
    let out = eval_str(
        "(() => { Array.prototype[1] = 1; \
         try { const x = [0]; x.length = 2; x.reverse(); \
         return (0 in x) + '|' + (1 in x) + '|' + x[0] + '|' + x[1]; } \
         finally { delete Array.prototype[1]; } })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|1|0");
}

// copyWithin 自拷贝：源洞位 DeleteProperty 自身，洞保洞。
#[test]
fn test_copy_within_self_copy_hole_stays_hole() {
    let out = eval_str(
        "(() => { const a = new Array(3); a[0] = 1; a[2] = 3; a.copyWithin(0); \
         return (1 in a) + '|' + a[0] + '|' + a[2]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|1|3");
}

// copyWithin 重叠正向陷阱：目标领先且重叠须反向迭代，源洞目标位删洞。
#[test]
fn test_copy_within_overlap_reverse_direction() {
    let out = eval_str(
        "(() => { const a = new Array(4); a[1] = 1; a[2] = 3; a[3] = 4; \
         a.copyWithin(1, 0, 2); \
         return (0 in a) + '|' + (1 in a) + '|' + (2 in a) + '|' + a[2]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|false|true|1");
}

// copyWithin 重叠方向判别：密集数组反向复制结果与正向不同。
#[test]
fn test_copy_within_dense_overlap() {
    let out = eval_str("(() => { const a = [0, 1, 2, 3]; a.copyWithin(1, 0, 2); return a.join(','); })()").unwrap();
    assert_eq!(out, "0,0,1,3");
}

// at 元素读走 Get：洞位落原型链，索引访问器触发。
#[test]
fn test_at_get_through_proto() {
    let out = eval_str(
        "(() => { Object.defineProperty(Array.prototype, 0, { get: () => 'AG' }); \
         try { const a = new Array(2); a[1] = 5; \
         return a.at(0) + '|' + a.at(-2) + '|' + a.at(1); } \
         finally { delete Array.prototype[0]; } })()",
    )
    .unwrap();
    assert_eq!(out, "AG|AG|5");
}

// reverse 读值期 getter 截断元素区：上端写越界扩展不得物化被删端，
// 终态被删端保洞、值端 present。
#[test]
fn test_reverse_getter_truncation_pair() {
    let out = eval_str(
        "(() => { const a = ['first', 'second']; \
         Object.defineProperty(a, 0, { get: () => { a.length = 0; return 'first'; } }); \
         a.reverse(); \
         return (0 in a) + '|' + (1 in a) + '|' + a[1] + '|' + a.length; })()",
    )
    .unwrap();
    assert_eq!(out, "false|true|first|2");
}

// reverse 读值期 getter 中间截断：上端写越界扩展的整段间隙 [old_count, upper)
// 补置洞，不只补被删端一洞。
#[test]
fn test_reverse_getter_mid_truncation_gaps_holes() {
    let out = eval_str(
        "(() => { const a = new Array(5); a[0] = 1; \
         Object.defineProperty(a, 0, { get: () => { a.length = 3; return 1; } }); \
         a.reverse(); \
         return (1 in a) + '|' + (3 in a) + '|' + a[4] + '|' + a.length; })()",
    )
    .unwrap();
    assert_eq!(out, "false|false|1|5");
}

// 防过度修护栏：fill 对洞位无条件 Set，洞位变 present-0，不改。
#[test]
fn test_fill_materializes_hole_unchanged() {
    let out = eval_str(
        "(() => { const a = new Array(3); a[0] = 1; a[2] = 3; a.fill(0); \
         return (1 in a) + '|' + a[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "true|0");
}

// at 越界返回 undefined（无原型链参与，防误改读路径的回归护栏）。
#[test]
fn test_at_out_of_bounds_undefined() {
    let (_vm, result) = eval_value("[1, 2].at(9)").unwrap();
    assert!(result.is_undefined());
}

// splice species：Ctor 返回体直接作结果，构造器恰收单参 actualDeleteCount，O 已搬移。
#[test]
fn test_splice_species_ctor_returned_instance() {
    let out = eval_str(
        "(() => { let seen = null; const inst = { mark: 1 }; \
         function Ctor(n) { seen = n; inst.len = n; return inst; } \
         const a = [0,1,2,3]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.splice(2); \
         return seen + '|' + (r === inst) + '|' + r.len + '|' + a.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "2|true|2|0,1");
}

// splice species 普通 Ctor 无返回体：结果非数组、原型为 Ctor.prototype，
// Set(A,"length") 扩长至删除数且值逐位填入。
#[test]
fn test_splice_species_plain_ctor_extends_length() {
    let out = eval_str(
        "(() => { function Ctor() {} \
         const a = [0,1,2,3]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.splice(0); \
         return Array.isArray(r) + '|' + (Object.getPrototypeOf(r) === Ctor.prototype) + '|' + \
         r.length + '|' + r[0] + ',' + r[1] + ',' + r[2] + ',' + r[3]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|true|4|0,1,2,3");
}

// splice species 为 null / 缺省：回退真数组（原型为 Array.prototype），值与长度正确。
#[test]
fn test_splice_species_null_undef_plain_array() {
    let out = eval_str(
        "(() => { const a = [0,1,2]; a.constructor = {}; a.constructor[Symbol.species] = null; \
         const r1 = a.splice(1); \
         const b = [0,1,2]; b.constructor = {}; \
         const r2 = b.splice(1); \
         return Array.isArray(r1) + '|' + (Object.getPrototypeOf(r1) === Array.prototype) + '|' + \
         r1.length + '|' + r1[0] + '|' + Array.isArray(r2) + '|' + \
         (Object.getPrototypeOf(r2) === Array.prototype) + '|' + r2.length; })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|2|1|true|true|2");
}

// splice species 构造器收 -0 折算的删除数时收到 +0（非 -0）。
#[test]
fn test_splice_species_neg_zero() {
    let out = eval_str(
        "(() => { let isNeg = null; \
         function Ctor(n) { isNeg = Object.is(n, -0); } \
         const a = []; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.splice(0, -0); \
         return 'neg=' + isNeg + '|len=' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "neg=false|len=0");
}

// splice species 结果 "0" 不可写：CreateDataPropertyOrThrow 覆盖为 present，
// 描述符恢复可写可配置。
#[test]
fn test_splice_species_writable_false_overwritten() {
    let out = eval_str(
        "(() => { function Ctor(n) { const o = {}; o.length = n; \
         Object.defineProperty(o, '0', { writable: false, configurable: true, enumerable: true, value: 99 }); \
         return o; } \
         const a = [0,1,2]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.splice(1); \
         const d = Object.getOwnPropertyDescriptor(r, '0'); \
         return r[0] + '|' + d.writable + '|' + d.configurable + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "1|true|true|2");
}

// splice species 结果 "0" 不可配置：CreateDataPropertyOrThrow 抛 TypeError。
#[test]
fn test_splice_species_non_configurable_throws() {
    let out = eval_str(
        "(() => { function Ctor(n) { const o = {}; o.length = n; \
         Object.defineProperty(o, '0', { writable: true, configurable: false, enumerable: true, value: 99 }); \
         return o; } \
         const a = [0,1,2]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         try { a.splice(1); return 'no-throw'; } catch (e) { return e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// splice species 结果不可扩展：CreateDataPropertyOrThrow 抛 TypeError。
#[test]
fn test_splice_species_non_extensible_throws() {
    let out = eval_str(
        "(() => { function Ctor(n) { const o = {}; o.length = n; Object.preventExtensions(o); return o; } \
         const a = [0,1,2]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         try { a.splice(1); return 'no-throw'; } catch (e) { return e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// splice constructor 为非对象非 undefined（null/数字/字符串/布尔四值）均抛 TypeError。
#[test]
fn test_splice_ctor_non_object_throws() {
    let out = eval_str(
        "(() => { let ok = 0; \
         for (const cv of [null, 1, 'string', true]) { \
           const a = [0,1,2]; a.constructor = cv; \
           try { a.splice(1); } catch (e) { if (e instanceof TypeError) ok++; } } \
         return String(ok); })()",
    )
    .unwrap();
    assert_eq!(out, "4");
}

// 防过度修：无 species 时结果仍为真数组、原型为 Array.prototype，自定义 Ctor 不被调用。
#[test]
fn test_splice_no_species_plain_array() {
    let out = eval_str(
        "(() => { let calls = 0; function Ctor() { calls++; } \
         const a = [0,1,2]; \
         const r = a.splice(1); \
         return Array.isArray(r) + '|' + (Object.getPrototypeOf(r) === Array.prototype) + '|' + r.length + '|' + calls; })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|2|0");
}

// 步骤序：构造器在收集与搬移之前执行，读到未改动的 O。
#[test]
fn test_splice_ctor_reads_o_before_mutation() {
    let out = eval_str(
        "(() => { let seenLen = null, seen0 = null; \
         function Ctor(n) { seenLen = a.length; seen0 = a[0]; } \
         const a = [0,1,2,3]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         a.splice(0, 2, 9); \
         return seenLen + '|' + seen0 + '|' + a.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "4|0|9,2,3");
}

// @@species getter 抛错原样透传（非构造器 constructor 上的 species 读取异常上抛）。
#[test]
fn test_splice_species_poisoned_propagates() {
    let out = eval_str(
        "(() => { const a = [0,1,2]; a.constructor = {}; \
         Object.defineProperty(a.constructor, Symbol.species, { get() { throw new RangeError('species-get'); } }); \
         try { a.splice(1); return 'no-throw'; } catch (e) { return e.constructor.name + ':' + e.message; } })()",
    )
    .unwrap();
    assert_eq!(out, "RangeError:species-get");
}

// length 不可写（语料同形 defineProperty 仅收窄 writable 位）：新旧长度相同的
// 空 splice 与收缩/插入 splice 均抛 TypeError。
#[test]
fn test_splice_length_nonwritable_throws() {
    let out = eval_str(
        "(() => { const a = [0,1,2]; Object.defineProperty(a, 'length', { writable: false }); \
         let k1; try { a.splice(1, 2, 4); return 'shrink-no-throw'; } catch (e) { k1 = e.constructor.name; } \
         const b = [0,1,2]; Object.defineProperty(b, 'length', { writable: false }); \
         try { b.splice(0, 0); return 'noop-no-throw'; } catch (e) { return k1 + '|' + e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError|TypeError");
}

// 冻结数组空删除也走 length 写：抛 TypeError（防空 splice 被特例放行）。
#[test]
fn test_splice_frozen_empty_delete_throws_length() {
    let out = eval_str(
        "(() => { try { Object.freeze([0,1,2]).splice(0, 0); return 'no-throw'; } \
         catch (e) { return e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 冻结数组删除命中元素：元素严格 Set 面抛 TypeError（回归护栏）。
#[test]
fn test_splice_frozen_element_set_throws() {
    let out = eval_str(
        "(() => { try { Object.freeze([0,1,2]).splice(1, 1); return 'no-throw'; } \
         catch (e) { return e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}
