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
