//! CONCAT_N 多操作数拼接（连续 `+` 左结合链摊平）语义回归测试。
//!
//! 覆盖：三形态混合锚点（数值前缀折叠 → 字符串模式单趟拼接）、BigInt 混合抛错点、
//! 对象 ToPrimitive 副作用顺序、Symbol 行为与二元 ADD 一致、f64 结合性（右括号不拆）、
//! 求值顺序、2 操作数与 3+ 操作数等价性、长链拼接正确性。

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {e}"))?;
    vm.run(&module)
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

/// 按 CLI 语义格式化结果：字符串取内容（不带引号），数值/BigInt 走 Display。
fn fmt(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        // SAFETY: val 已确认是字符串值。
        unsafe { (*val.as_string_ptr()).to_owned_string() }
    } else if val.is_bigint() {
        format!("{}", vm.bigint_value(val))
    } else {
        format!("{val}")
    }
}

fn eval_str(source: &str) -> String {
    let mut vm = Vm::new();
    let val = eval(&mut vm, source).expect("eval ok");
    fmt(&vm, val)
}

#[test]
fn concat_n_three_form_mixed_anchors() {
    // 数值前缀折叠保证 1+2+"3" = "33"（非 "123"）；字符串开头整链拼接；中缀字符串同理。
    assert_eq!(eval_str("1 + 2 + \"3\""), "33");
    assert_eq!(eval_str("\"1\" + 2 + 3"), "123");
    assert_eq!(eval_str("1 + \"2\" + 3"), "123");
    assert_eq!(eval_str("\"a\" + 1 + 2"), "a12");
}

#[test]
fn concat_n_pure_numeric_folds_like_left_assoc() {
    // 全数值链走阶段 1 折叠：int 溢出升 double 与两两 ADD 逐点一致。
    assert_eq!(eval_str("1 + 2 + 3 + 4 + 5"), "15");
    assert_eq!(eval_str("2147483647 + 1 + 2147483647"), "4294967295");
    assert_eq!(eval_str("1 + 2.5 + 3"), "6.5");
}

#[test]
fn concat_n_bigint_mix_semantics() {
    // 全 BigInt 链折叠；BigInt 混合在第一个混合点抛 TypeError（与 (1+2n) 同位置）；
    // BigInt + 字符串开头走字符串模式。
    assert_eq!(eval_str("1n + 2n + 3n"), "6");
    let err = eval(&mut Vm::new(), "1 + 2n + \"a\"").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got {err}");
    assert_eq!(eval_str("1n + \"a\" + 2"), "1a2");
}

#[test]
fn concat_n_object_valueof_side_effect_order() {
    // 操作数按源码序 coerce（左结合顺序），valueOf 副作用序保持。
    assert_eq!(
        eval_str(
            "var log = []; var o1 = { valueOf: function () { log.push(1); return 'x' } }; \
             var o2 = { valueOf: function () { log.push(2); return 'y' } }; \
             (o1 + o2 + 'z') + '|' + log.join(',')"
        ),
        "xyz|1,2"
    );
}

#[test]
fn concat_n_call_side_effect_order() {
    // f()+g()+h() 求值序 f→g→h（摊平后按序 emit，顺序保持）。
    assert_eq!(
        eval_str(
            "var log = []; \
             function f() { log.push('f'); return 'f' } \
             function g() { log.push('g'); return 'g' } \
             function h() { log.push('h'); return 'h' } \
             (f() + g() + h()) + '|' + log.join(',')"
        ),
        "fgh|f,g,h"
    );
}

#[test]
fn concat_n_symbol_matches_binary_add_behavior() {
    // Symbol 在字符串模式下被静默忽略（既有 concat_strings 行为），CONCAT_N 与二元
    // ADD 结果逐字节一致，不放大不修复。
    let flat = eval_str("var s = Symbol(); 'a' + s + 'b'");
    let binary = eval_str("var s = Symbol(); ('a' + s) + 'b'");
    assert_eq!(flat, binary);
    assert_eq!(flat, "ab");
}

#[test]
fn concat_n_right_parentheses_not_flattened() {
    // 右括号不拆：0.1+0.2+0.3 左结合（CONCAT_N）与 0.1+(0.2+0.3)（ADD 链）结果不同，
    // 证明摊平只发生在 left 链、右括号保持原结合性。
    assert_eq!(eval_str("0.1 + 0.2 + 0.3"), "0.6000000000000001");
    assert_eq!(eval_str("0.1 + (0.2 + 0.3)"), "0.6");
}

#[test]
fn concat_n_negative_double_appends() {
    // CONCAT_N 单趟拼接（≥3 操作数）：非空缓冲 + 负 double，负号追加在当前数前。
    assert_eq!(eval_str("'a' + 'b' + (-1.5)"), "ab-1.5");
    assert_eq!(eval_str("'a' + 'b' + (-1e21)"), "ab-1e+21");
    // ≥2^53 可精确表示整数负数（2^53+2）落 ryu 定点路径，追加语义保持一致。
    assert_eq!(eval_str("'a' + 'b' + (-9007199254740994)"), "ab-9007199254740994");
}

#[test]
fn concat_n_two_and_multi_operand_equivalence() {
    // 同一语义表达式以 2 操作数（ADD）与 3+ 操作数（CONCAT_N）两种写法输出一致。
    assert_eq!(eval_str("1 + 2 + 3"), eval_str("(1 + 2) + 3"));
    assert_eq!(eval_str("'a' + 'b' + 'c'"), eval_str("('a' + 'b') + 'c'"));
    assert_eq!(eval_str("1.5 + 2.5 + 3.5"), eval_str("(1.5 + 2.5) + 3.5"));
    assert_eq!(eval_str("1 + 2 + 3 + 4 + 5 + 6"), "21");
}

#[test]
fn concat_n_compound_rhs_chain() {
    // `s += a+b+c`：rhs 摊平为单条 CONCAT_N，COMPOUND_ADD 读旧值拼接；
    // 单操作数 rhs（`s += j`）不触达摊平，仍走 COMPOUND_ADD。
    assert_eq!(eval_str("var s = ''; s += 'a' + 'b' + 'c'; s"), "abc");
    assert_eq!(eval_str("var s = ''; for (var j = 0; j < 5; j++) { s += j } s"), "01234");
}

#[test]
fn concat_n_large_string_builds_correctly() {
    // 长串多段拼接：长度与逐字符位置正确（结果串单趟成型）。
    assert_eq!(eval_str("('x'.repeat(1000) + 'a' + 'b' + 'c').length"), "1003");
    assert_eq!(eval_str("('x'.repeat(1000) + 'a' + 'b' + 'c').charCodeAt(1000)"), "97");
    assert_eq!(eval_str("('x'.repeat(1000) + 'a' + 'b' + 'c').charCodeAt(1002)"), "99");
}

#[test]
fn concat_n_single_and_two_operand_degrades() {
    // 1/2 操作数不触达 CONCAT_N（emit 层退化 ADD），行为不变。
    let mut vm = Vm::new();
    assert_eq!(eval(&mut vm, "1").unwrap().as_int(), 1);
    assert_eq!(eval(&mut vm, "1 + 2").unwrap().as_int(), 3);
    let ab = eval(&mut vm, "\"a\" + \"b\"").unwrap();
    assert_eq!(to_str(&vm, ab), "ab");
}

#[test]
fn concat_n_emit_registers_bounded_for_long_chain() {
    // 超长纯链部分摊平：CONCAT_N 摊平上限保证单点同时存活寄存器 ≤15，纯表达式
    // 链的寄存器复用属性保持（编译不因摊平而耗尽寄存器）。
    let allocator = oxide_parser::Allocator::default();
    let source = std::iter::repeat("1").take(40).collect::<Vec<_>>().join(" + ");
    let program = oxide_parser::parse(&allocator, &source).expect("parse ok");
    let module = Compiler::new().compile(&program).expect("compile ok");
    assert!(module.n_registers < 16, "long chain registers {}, expected < 16", module.n_registers);
}
