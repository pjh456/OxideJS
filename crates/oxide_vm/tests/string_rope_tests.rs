//! 二元 ConsString（rope）语义回归测试。
//!
//! 覆盖：`+`/`+=` 拼接产物（rope 节点）在各消费点（`==`/`.length`/string 方法/
//! for-of/属性键/模板/JSON/展开）的惰性扁平化正确性、数字/对象叶子混合、CONCAT_N
//! 产物作 rope 叶子、低 GC 阈值下跨执行期字符串回收的存活与 mark 传播。

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {e}"))?;
    vm.run(&module)
}

fn eval_str(source: &str) -> String {
    let mut vm = Vm::new();
    let val = eval(&mut vm, source).expect("eval ok");
    fmt(&vm, val)
}

fn fmt(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        vm.lookup_str(val).unwrap_or_default()
    } else if val.is_bigint() {
        format!("{}", vm.bigint_value(val))
    } else {
        format!("{val}")
    }
}

/// 低 GC 阈值 VM：水位=1 使指令边界检查几乎每指令触发，压 rope 的 mark 传播与
/// 跨回收存活路径。
fn vm_with_tiny_threshold() -> Vm {
    let mut cfg = KernelConfig::minimal();
    cfg.set_session_gc_threshold(1);
    let core = KernelCore::new(cfg);
    Vm::with_kernel_core(core)
}

#[test]
fn rope_binary_concat_content_and_length() {
    // 二元 `+` 产物为 Cons 节点：内容与长度经惰性扁平化与缓存 O(1) 读取。
    assert_eq!(eval_str("\"ab\" + \"cd\""), "abcd");
    assert_eq!(eval_str("(\"ab\" + \"cd\").length"), "4");
    assert_eq!(eval_str("(\"ab\" + \"cd\" + \"ef\").length"), "6");
    assert_eq!(eval_str("(\"ab\" + \"cd\").charCodeAt(1)"), "98");
}

#[test]
fn rope_equality_with_literal_and_rope() {
    // `==`/`===` 走 string_value_eq 内容比较：rope 与同内容字面量/另一 rope 相等，
    // 与不同内容不等。
    assert_eq!(eval_str("(\"ab\" + \"cd\") === \"abcd\""), "true");
    assert_eq!(eval_str("(\"ab\" + \"cd\") == \"abcd\""), "true");
    assert_eq!(eval_str("(\"ab\" + \"cd\") === (\"a\" + \"bcd\")"), "true");
    assert_eq!(eval_str("(\"ab\" + \"cd\") === \"abce\""), "false");
}

#[test]
fn rope_compound_loop_builds_correctly() {
    // string_build 主模式：`s += j` 循环 50 次（数字叶子 + Cons 链接），
    // 末次 `.length` 扁平化结果正确（0..49 拼接：10 个单字符 + 40 个双字符 = 90）。
    assert_eq!(
        eval_str(
            "var s = ''; for (var j = 0; j < 50; j++) { s += j } \
             s + '|' + s.length"
        ),
        "012345678910111213141516171819202122232425262728293031323334353637383940414243444546474849|90"
    );
}

#[test]
fn rope_mixed_operand_leaf_conversion() {
    // 非字符串操作数先转叶子再链接：数字/布尔/null/undefined/对象与字符串混拼。
    assert_eq!(eval_str("var s = 'a'; s += 1; s += 2.5; s += true; s"), "a12.5true");
    assert_eq!(eval_str("'x' + null + undefined"), "xnullundefined");
    assert_eq!(eval_str("var s = 'a'; s += { toString: function () { return 'obj' } }; s"), "aobj");
}

#[test]
fn rope_concat_n_rhs_becomes_leaf() {
    // `s += a+b+c`：rhs 摊平为 CONCAT_N（急切 Flat 产物），外层 COMPOUND_ADD 把
    // Flat 产物链接成 Cons 子节点——两层机制叠加内容正确。
    assert_eq!(eval_str("var s = ''; s += 'a' + 'b' + 'c'; s + s"), "abcabc");
    assert_eq!(eval_str("var s = 'z'; for (var i = 0; i < 5; i++) { s += 'x' + 'y' } s"), "zxyxyxyxyxy");
}

#[test]
fn rope_as_property_key() {
    // rope 作属性键：ToPropertyKey → flat 桥接 intern，键内容一致。
    assert_eq!(eval_str("var s = 'a' + 'b'; var o = {}; o[s] = 42; o['ab']"), "42");
    assert_eq!(eval_str("var s = '5'; var o = {}; o[s] = 7; o[5]"), "7");
}

#[test]
fn rope_for_of_iterates_chars() {
    // for-of 字符串迭代 rope：首次访问扁平化后游标推进，逐字符正确。
    assert_eq!(eval_str("var s = 'ab' + 'cd'; var out = ''; for (var c of s) { out += c } out"), "abcd");
    assert_eq!(eval_str("var s = 'x'; s += 'y'; s += 'z'; [...s].join('-')"), "x-y-z");
}

#[test]
fn rope_consumed_by_string_methods() {
    // string 方法族（this_string 借用 + 只读扫描）消费 rope 时惰性扁平化。
    assert_eq!(eval_str("('hello' + 'world').indexOf('wo')"), "5");
    assert_eq!(eval_str("('hello' + 'world').substring(2, 7)"), "llowo");
    assert_eq!(eval_str("('ab' + 'cd').repeat(2)"), "abcdabcd");
    assert_eq!(eval_str("('aB' + 'cD').toUpperCase()"), "ABCD");
    assert_eq!(eval_str("('a,b' + ',c').split(',').length"), "3");
}

#[test]
fn rope_in_template_and_json() {
    // 模板串与 JSON.stringify 消费 rope 结果一致。
    assert_eq!(eval_str("var s = 'a' + 'b'; `${s}-x`"), "ab-x");
    assert_eq!(eval_str("var s = 'x' + 'y'; JSON.stringify([s])"), "[\"xy\"]");
}

#[test]
fn rope_numeric_coercion_of_content() {
    // rope 文本参与数值转换：ToNumber 走 string_data 扁平化文本。
    assert_eq!(eval_str("('1' + '0') * 2"), "20");
    assert_eq!(eval_str("Number('4' + '2')"), "42");
}

#[test]
fn rope_survives_runtime_string_gc() {
    // 低阈值下指令边界检查几乎每指令触发：循环拼接全程（链接结果写寄存器先于
    // 下次检查）不被误回收；大量中间垃圾触发多轮回收后 rope 仍正确消费。
    let mut vm = vm_with_tiny_threshold();
    let val = eval(
        &mut vm,
        "var s = 'a'; for (var i = 0; i < 300; i++) { s += 'b' } \
         var garbage = ''; for (var g = 0; g < 500; g++) { garbage += 'x' } \
         s.length + '|' + s.charAt(150)",
    )
    .expect("eval ok");
    assert_eq!(fmt(&vm, val), "301|b");
}

#[test]
fn rope_in_object_holder_survives_gc() {
    // rope 挂在全局对象属性上：mark 经对象字符串边传播 rope 闭包，子节点与
    // 产物随父存活（跨多轮低阈值回收）。
    let mut vm = vm_with_tiny_threshold();
    let val = eval(
        &mut vm,
        "var o = {}; o.s = 'x'; for (var i = 0; i < 200; i++) { o.s += 'y' } \
         var t = ''; for (var g = 0; g < 300; g++) { t += 'z' } \
         o.s.length + '|' + o.s.charAt(200) + '|' + t.length",
    )
    .expect("eval ok");
    assert_eq!(fmt(&vm, val), "201|y|300");
}

#[test]
fn rope_dead_after_scope_released() {
    // rope 离开作用域成为垃圾：低阈值下被后续分配回收，不影响新值。
    let mut vm = vm_with_tiny_threshold();
    eval(&mut vm, "var t = ''; for (var i = 0; i < 200; i++) { t += 'z' }").expect("eval ok");
    let val = eval(&mut vm, "'fresh' + 'value'").expect("eval ok");
    assert_eq!(fmt(&vm, val), "freshvalue");
}
