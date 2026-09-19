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

// 引擎钉：splice 长度源按 ToLength 读取，2^53 及以上钳位到 2^53-1。
#[test]
fn test_splice_length_clamped_to_integer_limit() {
    let out = eval_str(
        "(() => { const o = {}; o.length = 9007199254740992; \
         Array.prototype.splice.call(o); return String(o.length); })()",
    )
    .unwrap();
    assert_eq!(out, "9007199254740991");
}

// 引擎钉：splice 新长度（len + insertCount - actualDeleteCount）超 2^53-1 抛 TypeError。
#[test]
fn test_splice_new_length_over_integer_limit_throws() {
    let out = eval_str(
        "(() => { const o = {}; o.length = 9007199254740991; \
         try { Array.prototype.splice.call(o, 0, 0, null); return 'no-throw'; } \
         catch (e) { return e.constructor.name; } })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 引擎钉：splice 极限扩容对——arraylike 上元素右移一位、插入位落值、长度扩到 2^53-1。
#[test]
fn test_splice_integer_limit_grow_pair() {
    let out = eval_str(
        "(() => { const o = { '9007199254740985': '9007199254740985', \
         '9007199254740986': '9007199254740986', '9007199254740987': '9007199254740987', \
         '9007199254740989': '9007199254740989', '9007199254740991': '9007199254740991', \
         length: 9007199254740990 }; \
         const r = Array.prototype.splice.call(o, 9007199254740986, 0, 'new-value'); \
         return r.length + '|' + o.length + '|' + o['9007199254740986'] + '|' + \
         o['9007199254740987'] + '|' + o['9007199254740988'] + '|' + \
         ('9007199254740989' in o) + '|' + o['9007199254740990'] + '|' + o['9007199254740991']; })()",
    )
    .unwrap();
    assert_eq!(
        out,
        "0|9007199254740991|new-value|9007199254740986|9007199254740987|false|9007199254740989|9007199254740991"
    );
}

// 引擎钉：splice 极限收缩对——搬移 + 尾部删位 + 长度缩到 2^53-2。
#[test]
fn test_splice_integer_limit_shrink_pair() {
    let out = eval_str(
        "(() => { const o = { '9007199254740986': '9007199254740986', \
         '9007199254740987': '9007199254740987', '9007199254740988': '9007199254740988', \
         '9007199254740990': '9007199254740990', '9007199254740991': '9007199254740991', \
         length: 9007199254740992 }; \
         const r = Array.prototype.splice.call(o, 9007199254740987, 1); \
         return r.length + '|' + r[0] + '|' + o.length + '|' + o['9007199254740986'] + '|' + \
         o['9007199254740987'] + '|' + ('9007199254740988' in o) + '|' + \
         o['9007199254740989'] + '|' + ('9007199254740990' in o) + '|' + o['9007199254740991']; })()",
    )
    .unwrap();
    assert_eq!(out, "1|9007199254740987|9007199254740990|9007199254740986|9007199254740988|false|9007199254740990|false|9007199254740991");
}

// 引擎钉：splice 无参——length getter 恰读一次、setter 恰写一次且值为 0。
#[test]
fn test_splice_no_args_length_get_set_once() {
    let out = eval_str(
        "(() => { let g = 0, s = 0, v = null; \
         const o = { get length() { g += 1; return '0'; }, \
          set length(x) { s += 1; v = x; } }; \
         Array.prototype.splice.call(o); return g + '|' + s + '|' + v; })()",
    )
    .unwrap();
    assert_eq!(out, "1|1|0");
}

// 引擎钉：reverse 在超大 arraylike 上首对即触发上端 getter（StopReverse 上抛）。
#[test]
fn test_reverse_integer_limit_object_throws_on_first_pair() {
    let out = eval_str(
        "(() => { function StopReverse() {} \
         const o = { get '9007199254740990'() { throw new StopReverse(); }, \
          length: 9007199254740994 }; \
         try { Array.prototype.reverse.call(o); return 'no-throw'; } \
         catch (e) { return String(e instanceof StopReverse); } })()",
    )
    .unwrap();
    assert_eq!(out, "true");
}

// 引擎钉：copyWithin 极限区间——源缺失位目标删位、present 位跨极大下标拷贝。
#[test]
fn test_copy_within_integer_limit_range() {
    let out = eval_str(
        "(() => { const si = 9007199254740988; \
         const o = { 0: 0, 1: 1, 2: 2, length: 9007199254740992 }; \
         o[si] = -3; o[si + 2] = -1; \
         Array.prototype.copyWithin.call(o, 0, si, si + 3); \
         return o[0] + '|' + (1 in o) + '|' + o[2]; })()",
    )
    .unwrap();
    assert_eq!(out, "-3|false|-1");
}

// 引擎钉：fill 极限区间三格全落值；setter 抛错原样上抛（严格 Set 面）。
#[test]
fn test_fill_integer_limit_range_and_setter_throws() {
    let out = eval_str(
        "(() => { const si = 9007199254740988; const v = { m: 1 }; \
         const o = { length: 9007199254740992 }; \
         Array.prototype.fill.call(o, v, si, si + 3); \
         const a2 = { length: 1 }; \
         Object.defineProperty(a2, '0', { set: function () { throw new RangeError('set'); } }); \
         let th = null; try { Array.prototype.fill.call(a2); } catch (e) { th = e.constructor.name; } \
         return (o[si] === v) + '|' + (o[si + 1] === v) + '|' + (o[si + 2] === v) + '|' + th; })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|true|RangeError");
}

// 引擎钉：at 负索引自逻辑长度（稀疏覆盖值）折算，越界返回 undefined。
#[test]
fn test_at_negative_index_uses_logical_length() {
    let out = eval_str(
        "(() => { const a = new Array(1); a.length = 2147483653; a[0] = 'x'; \
         const b = new Array(1); b.length = 2147483649; \
         return a.at(-2147483653) + '|' + String(b.at(2147483648) === undefined); })()",
    )
    .unwrap();
    assert_eq!(out, "x|true");
}

// 引擎钉：at 极负索引 k = len + relative < 0 返 undefined（不钳零回首元素）；
// at(-len) 为首元素、at(-len-1) 为 undefined。
#[test]
fn test_at_extreme_negative_index_undefined() {
    let out = eval_str(
        "(() => { const a = [7, 8]; \
         return String(a.at(-1e20) === undefined) + '|' + a.at(-2) + '|' + String(a.at(-3) === undefined); })()",
    )
    .unwrap();
    assert_eq!(out, "true|7|true");
}

// 引擎钉：基元 this 经 ToObject 装箱——reverse 返回 Boolean 包装体、splice 返回真数组。
#[test]
fn test_splice_reverse_primitive_this_boxed() {
    let out = eval_str(
        "(() => { const r1 = Array.prototype.reverse.call(true); \
         const r2 = Array.prototype.splice.call(false); \
         return (r1 instanceof Boolean) + '|' + Array.isArray(r2) + '|' + r2.length; })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|0");
}

// 引擎钉：concat 空数组实参按 ConcatSteps 展开为零元素——不贡献元素也不占位。
#[test]
fn test_concat_empty_array_arg_expands_zero_elements() {
    let out = eval_str(
        "(() => { return [1].concat([]).length + '|' + [1].concat([], []).length + '|' + \
         [].concat([]).length + '|' + [1, 2].concat(new Array(0)).length; })()",
    )
    .unwrap();
    assert_eq!(out, "1|1|0|2");
}

// 引擎钉：concat 多实参混合 present/空数组/非数组实参展开序与规范一致，
// 空数组实参中间穿插不占位（length 观察面钉死）；洞实参位保洞。
#[test]
fn test_concat_mixed_args_order_and_holes() {
    let out = eval_str(
        "(() => { const mixed = [].concat([], 5, []); \
         const ordered = [1].concat([2, 3], 0, []); \
         const r = [].concat(new Array(1)); \
         return mixed.join(',') + '|' + mixed.length + '|' + ordered.join(',') + '|' + \
          ordered.length + '|' + r.length + '|' + (0 in r); })()",
    )
    .unwrap();
    assert_eq!(out, "5|1|1,2,3,0|4|1|false");
}

// ── flat / flatMap 收口钉（node v20.19.2 实测值钉死） ──────────────────────

// 引擎钉：reverse (S,N) 臂非可配置 lower——Delete 先于 Set，抛时零改动。
#[test]
fn test_reverse_sn_non_configurable_lower_throws_untouched() {
    let out = eval_str(
        "(() => { const a = [1, 2]; delete a[1]; \
         Object.defineProperty(a, '0', { value: 1, writable: true, enumerable: true, configurable: false }); \
         let threw = false; try { a.reverse(); } catch (e) { threw = e.name === 'TypeError'; } \
         return threw + '|' + (1 in a) + '|' + a[0] + '|' + a[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "true|false|1|undefined");
}

// 引擎钉：reverse (N,S) 臂非可配置 upper——Set(lower) 先成功、Delete(upper) 抛，
// 部分改动形态保留（判别力核心）。
#[test]
fn test_reverse_ns_non_configurable_upper_partial_mutation() {
    let out = eval_str(
        "(() => { const a = [undefined, 2, 3]; delete a[0]; \
         Object.defineProperty(a, '2', { value: 3, writable: true, enumerable: true, configurable: false }); \
         let threw = false; try { a.reverse(); } catch (e) { threw = e.name === 'TypeError'; } \
         return threw + '|' + a[0] + '|' + a[1] + '|' + a[2]; })()",
    )
    .unwrap();
    assert_eq!(out, "true|3|2|3");
}

// 引擎钉：reverse 读序 Hf,Gf,Ht,Gt 逐对交错 + (S,S) 臂写序 S0,S3,S1,S2；
// getter 还原原值，终读经 getter 呈现原序列（读序钉位核心）。
// 访问器逐条显式定义（不走 for 循环闭包捕获，规避既有作用域面）。
#[test]
fn test_reverse_get_set_order_and_final_values() {
    let out = eval_str(
        "(() => { const a = [10, 11, 22, 30]; const log = []; \
         Object.defineProperty(a, 0, { configurable: true, get() { log.push('G0'); return 10; }, set(x) { log.push('S0=' + x); } }); \
         Object.defineProperty(a, 1, { configurable: true, get() { log.push('G1'); return 11; }, set(x) { log.push('S1=' + x); } }); \
         Object.defineProperty(a, 2, { configurable: true, get() { log.push('G2'); return 22; }, set(x) { log.push('S2=' + x); } }); \
         Object.defineProperty(a, 3, { configurable: true, get() { log.push('G3'); return 30; }, set(x) { log.push('S3=' + x); } }); \
         a.reverse(); return log.join(',') + '|' + a.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "G0,G3,S0=30,S3=10,G1,G2,S1=22,S2=11|10,11,22,30");
}

// 引擎钉：reverse getter 内 length=0 截断（GID 形态）——(S,N) 臂 Delete(lower)
// 后 upper 写越界扩长，`0 in` 假、`1 in` 真、a[1] 为下值。
#[test]
fn test_reverse_getter_length_zero_in_check() {
    let out = eval_str(
        "(() => { const array = ['first', 'second']; \
         Object.defineProperty(array, 0, { get() { array.length = 0; return 'first'; } }); \
         array.reverse(); return (0 in array) + '|' + (1 in array) + '|' + array[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "false|true|first");
}

// 引擎钉：flat arraylike 接收者——ToObject 通用入口 + LengthOfArrayLike 长度源，
// 嵌套数组元素展开一层。
#[test]
fn test_flat_arraylike_receiver_nested() {
    let out = eval_str(
        "(() => { const r = Array.prototype.flat.call({ length: 2, 0: [1], 1: [2] }); \
         return r.join(',') + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "1,2|2");
}

// 引擎钉：flat arraylike 分数长度——ToLength 折算 2.9 → 2，第三元素不入。
#[test]
fn test_flat_arraylike_fractional_length() {
    let out = eval_str(
        "(() => { const r = Array.prototype.flat.call({ length: 2.9, 0: 1, 1: 2, 2: 3 }); \
         return r.join(',') + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "1,2|2");
}

// 引擎钉：flat arraylike NaN 长度——ToLength(NaN) = 0，空结果。
#[test]
fn test_flat_arraylike_nan_length() {
    let out = eval_str(
        "(() => { const r = Array.prototype.flat.call({ length: NaN, 0: 1 }); \
         return r.join(',') + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "|0");
}

// 引擎钉：flat arraylike undefined 长度——Get length 缺省 undefined，ToLength 0。
#[test]
fn test_flat_arraylike_undefined_length() {
    let out = eval_str(
        "(() => { const r = Array.prototype.flat.call({ length: undefined, 0: [1] }); \
         return r.join(',') + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "|0");
}

// 引擎钉：flat 装箱基元 this——ToObject 后长度为 0，结果空真数组。
#[test]
fn test_flat_boxed_primitive_this() {
    let out = eval_str(
        "(() => { const r = Array.prototype.flat.call(true); \
         return r.length + '|' + Array.isArray(r); })()",
    )
    .unwrap();
    assert_eq!(out, "0|true");
}

// 引擎钉：flat 负 depth——ToIntegerOrInfinity 后归 0，不展开。
#[test]
fn test_flat_negative_depth_no_flatten() {
    assert_eq!(eval_str("(() => { return JSON.stringify([[1]].flat(-1)); })()").unwrap(), "[[1]]");
}

// 引擎钉：flat NaN depth——ToIntegerOrInfinity(NaN) = 0，不展开（非默认深 1）。
#[test]
fn test_flat_nan_depth_no_flatten() {
    assert_eq!(eval_str("(() => { return JSON.stringify([[1]].flat(NaN)); })()").unwrap(), "[[1]]");
}

// 引擎钉：flat 0 depth——0 不展开（旧 (n as i32).max(1) 形态会误展开一层）。
#[test]
fn test_flat_zero_depth_no_flatten() {
    assert_eq!(eval_str("(() => { return JSON.stringify([[1]].flat(0)); })()").unwrap(), "[[1]]");
}

// 引擎钉：flat +∞ depth——无限深度臂，全嵌套展开。
#[test]
fn test_flat_infinity_depth() {
    let out = eval_str("(() => { return [1, [2, [3, [4]]]].flat(Infinity).join(','); })()").unwrap();
    assert_eq!(out, "1,2,3,4");
}

// 引擎钉：flat 超大 depth（i32 回绕域）——f64 域无回绕，浅数组上等效深 1。
#[test]
fn test_flat_huge_depth_no_wrap() {
    assert_eq!(eval_str("(() => { return JSON.stringify([[1]].flat(3000000000)); })()").unwrap(), "[1]",);
}

// 引擎钉：flat Symbol / null 原型对象 depth——ToPrimitive 转换异常按规范传播
// TypeError（旧 unwrap_or 吞错形态会静默返空）。
#[test]
fn test_flat_symbol_null_proto_depth_throws() {
    let out = eval_str(
        "(() => { let t1 = '?'; let t2 = '?'; \
         try { [1].flat(Symbol()); } catch (e) { t1 = e.name; } \
         try { [1].flat(Object.create(null)); } catch (e) { t2 = e.name; } \
         return t1 + '|' + t2; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError|TypeError");
}

// 引擎钉：flat species frozen 目标——CreateDataPropertyOrThrow 抛 TypeError。
#[test]
fn test_flat_species_frozen_target_throws() {
    let out = eval_str(
        "(() => { const a = [1, 2]; a.constructor = {}; \
         a.constructor[Symbol.species] = (n) => Object.freeze([]); \
         let t = '?'; try { a.flat(); } catch (e) { t = e.name; } return t; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 引擎钉：flat species 构造器为 null——ArraySpeciesCreate 步 9 抛 TypeError。
#[test]
fn test_flat_species_null_ctor_throws() {
    let out = eval_str(
        "(() => { const a = [1, 2]; a.constructor = null; \
         let t = '?'; try { a.flat(); } catch (e) { t = e.name; } return t; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 引擎钉：flat species 非可写 '0' 槽（configurable）——CDO 覆写并恢复可写描述符；
// 普通对象目标 length 为普通属性，不随 CDO 扩长。
#[test]
fn test_flat_species_non_writable_slot_overwritten() {
    let out = eval_str(
        "(() => { function Ctor(n) { const o = {}; o.length = n; \
         Object.defineProperty(o, '0', { writable: false, configurable: true, enumerable: true, value: 99 }); \
         return o; } \
         const a = [0, 1]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.flat(); const d = Object.getOwnPropertyDescriptor(r, '0'); \
         return r[0] + '|' + d.writable + '|' + d.configurable + '|' + r.length; })()",
    )
    .unwrap();
    assert_eq!(out, "0|true|true|0");
}

// 引擎钉：flat 洞语义——HasProperty 门控丢洞，结果恒紧凑（防误修回归）。
#[test]
fn test_flat_holes_dropped_compact() {
    let out = eval_str(
        "(() => { const r1 = [1, , 2].flat(); const r2 = [[1, , 2], [3]].flat(); \
         return r1.join(',') + '|' + r1.length + '|' + r2.join(',') + '|' + r2.length; })()",
    )
    .unwrap();
    assert_eq!(out, "1,2|2|1,2,3|3");
}

// 引擎钉：flat 真数组 length 增/减后——长度源为 ToLength(Get length)，洞位丢洞。
#[test]
fn test_flat_logical_length_grow_shrink() {
    let out = eval_str(
        "(() => { const a = [1, 2, 3]; a.length = 5; const r1 = a.flat(); \
         const b = [1, 2, 3]; b.length = 1; const r2 = b.flat(); \
         return r1.join(',') + '|' + r1.length + '|' + r2.join(',') + '|' + r2.length; })()",
    )
    .unwrap();
    assert_eq!(out, "1,2,3|3|1|1");
}

// 引擎钉：flat 非数组嵌套元素原样保留（防过度展开：仅真数组递归）。
#[test]
fn test_flat_non_array_elements_kept() {
    let out = eval_str(
        "(() => { const o = { a: 1 }; const r = [[o], 2].flat(); \
         return r.length + '|' + (r[0] === o) + '|' + r[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "2|true|2");
}

// 引擎钉：flatMap ToObject 通用入口——装箱基元 this（旧无装箱形态对基元抛
// TypeError）长度为 0 返空真数组；arraylike 接收者形态同入口，回调结果
// 真数组展开一层。
#[test]
fn test_flat_map_to_object_entry() {
    let out = eval_str(
        "(() => { const r1 = Array.prototype.flatMap.call(5, (v) => v); \
         const r2 = Array.prototype.flatMap.call({ length: 1, 0: 5 }, (v) => [v, v + 1]); \
         return r1.length + '|' + Array.isArray(r1) + '|' + r2.join(',') + '|' + r2.length; })()",
    )
    .unwrap();
    assert_eq!(out, "0|true|5,6|2");
}

// 引擎钉：flatMap species——ctor 传播结果构造（旧无物种裸构造形态不走 ctor，
// tag 缺位、ctor 日志空）。
#[test]
fn test_flat_map_species_ctor_propagates() {
    let out = eval_str(
        "(() => { const log = []; function Ctor(n) { log.push('c' + n); const a = []; a.tag = 'X'; return a; } \
         const a = [1, 2]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.flatMap((v) => [v]); \
         return r.join(',') + '|' + r.length + '|' + r.tag + '|' + log.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "1,2|2|X|c0");
}

// 引擎钉：flatMap species frozen 目标——CreateDataPropertyOrThrow 抛 TypeError
// （旧裸写不抛形态静默落空不抛）。
#[test]
fn test_flat_map_species_frozen_target_throws() {
    let out = eval_str(
        "(() => { const a = [1, 2]; a.constructor = {}; \
         a.constructor[Symbol.species] = (n) => Object.freeze([]); \
         let t = '?'; try { a.flatMap((v) => [v]); } catch (e) { t = e.name; } return t; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 引擎钉：flatMap 回调结果洞源——展开长度源为 LengthOfArrayLike，洞位不入
// 目标（旧 prop_count 长度源形态少读洞后存在位，结果短一元素）。
#[test]
fn test_flat_map_hole_source_length_of_arraylike() {
    let out = eval_str(
        "(() => { const r = [1, 2].flatMap((v) => { const x = new Array(2); x[1] = v; return x; }); \
         return r.join(',') + '|' + r.length + '|' + (0 in r) + '|' + (1 in r); })()",
    )
    .unwrap();
    assert_eq!(out, "1,2|2|true|true");
}

// 引擎钉：species 构造器为 undefined 时不查 species，四挂载点（slice/splice/
// flat/flatMap）均回退内置 ArrayCreate——纯数组、原型 Array.prototype、元素逐值。
#[test]
fn test_species_ctor_undefined_plain_array_four_mounts() {
    let out = eval_str(
        "(() => { const mk = () => { const a = [1, 2, 3]; a.constructor = undefined; return a; }; \
         const s = mk().slice(); const sp = mk().splice(1, 1); \
         const f = mk().flat(); const fm = mk().flatMap((x) => x); \
         return Array.isArray(s) + '|' + (Object.getPrototypeOf(s) === Array.prototype) + '|' + \
         s.join(',') + '|' + Array.isArray(sp) + '|' + sp.join(',') + '|' + \
         Array.isArray(f) + '|' + f.join(',') + '|' + Array.isArray(fm) + '|' + fm.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|1,2,3|true|2|true|1,2,3|true|1,2,3");
}

// 防过度修护栏：species 构造器为 null/数字/字符串/布尔四值时，四挂载点均保持
// 抛 TypeError（仅 undefined 判定前移，null/基元臂不得回退）。
#[test]
fn test_species_ctor_null_primitive_throws_four_mounts() {
    let out = eval_str(
        "(() => { let ok = 0; \
         for (const cv of [null, 1, 'string', true]) { \
           const a = [1, 2, 3]; a.constructor = cv; \
           try { a.slice(); } catch (e) { if (e instanceof TypeError) ok++; } \
           const b = [1, 2, 3]; b.constructor = cv; \
           try { b.splice(1); } catch (e) { if (e instanceof TypeError) ok++; } \
           const c = [1, 2, 3]; c.constructor = cv; \
           try { c.flat(); } catch (e) { if (e instanceof TypeError) ok++; } \
           const d = [1, 2, 3]; d.constructor = cv; \
           try { d.flatMap((x) => x); } catch (e) { if (e instanceof TypeError) ok++; } } \
         return String(ok); })()",
    )
    .unwrap();
    assert_eq!(out, "16");
}
