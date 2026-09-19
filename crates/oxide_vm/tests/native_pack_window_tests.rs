//! native pack 实参区 × 恢复边界镜像重载的引擎钉：native pack 协议在飞
//! （`.call`/`.apply` 转发、直接成员调用）期间，pack 与实参读取之间若存在用户
//! 内联窗（length getter / 回调），窗口拷回后的调用方模块镜像重载会把顶层
//! 镜像名集（Array/Object 着落于 pack 区寄存器）重写回寄存器，覆写外层
//! builtin 尚未读取的实参值。期望值一律取 node v20.19.2 实测值。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

/// 运行脚本并取最终值（钉的期望值统一为字符串形态）。
fn eval_str(source: &str) -> String {
    let module = compile(source);
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).expect("结果应为字符串").to_string()
}

// 共用的 length getter 装置：基元/对象 this 的 length 走用户 accessor，
// accessor 内联窗在 pack 与实参读取之间打开。
const LENGTH_GETTER: &str = "var savedLen = 1; \
      Object.defineProperty(Number.prototype, 'length', { \
        configurable: true, \
        get: function () { return savedLen; }, \
        set: function (v) { savedLen = v; } }); \
      ";

/// 在 body 脚本前拼入 length getter 装置，返回完整脚本。
fn with_length_getter(body: &str) -> String {
    format!("{LENGTH_GETTER}{body}")
}

/// `.call` 转发：splice 的 deleteCount 落入 reg 1（顶层镜像着色域），
/// length getter 内联窗先于实参读取，窗口拷回后的镜像重载不得覆写实参区。
#[test]
fn splice_call_length_getter_keeps_delete_count() {
    assert_eq!(
        eval_str(&with_length_getter(
            "var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        )),
        "1|2"
    );
}

/// 对象 this 同形态：装箱非触发条件，覆写对对象接收者同样成立。
#[test]
fn splice_object_this_length_getter_keeps_delete_count() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             var o = {}; \
             Object.defineProperty(o, 'length', { \
               configurable: true, \
               get: function () { return savedLen; }, \
               set: function (v) { savedLen = v; } }); \
             var r = Array.prototype.splice.call(o, 0, 1, 'a', 'b'); \
             [r.length, savedLen].join('|')"
        ),
        "1|2"
    );
}

/// 最小 getter 体（无副作用）：覆写与 getter 复杂度无关，是最小判别形。
#[test]
fn splice_minimal_getter_keeps_delete_count() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             Object.defineProperty(Number.prototype, 'length', { \
               configurable: true, \
               get: function () { return 1; }, \
               set: function (v) { savedLen = v; } }); \
             var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        ),
        "1|2"
    );
}

/// 2 实参 pack 区 [0, 2) 恰好覆盖第一个镜像寄存器：deleteCount 读值不得失真。
#[test]
fn splice_two_arg_pack_keeps_delete_count() {
    assert_eq!(
        eval_str(&with_length_getter(
            "var r = Array.prototype.splice.call(5, 0, 1); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        )),
        "1|0"
    );
}

/// 大 getter 体（字符串 churn、寄存器需求上升）：覆写与窗宽无关，窗宽充分。
#[test]
fn splice_churn_getter_keeps_delete_count() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             Object.defineProperty(Number.prototype, 'length', { \
               configurable: true, \
               get: function () { \
                 var s = ''; \
                 for (var i = 0; i < 200; i++) { s = s + 'churn' + i; } \
                 globalThis.__churn = s; \
                 return savedLen; \
               }, \
               set: function (v) { savedLen = v; } }); \
             var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        ),
        "1|2"
    );
}

/// fill 的 start/end 落 pack 区 reg 1/2：区间两端读值不得折算失真，
/// 填充范围与未填充位保持 node 语义（length getter 开窗）。
#[test]
fn fill_arraylike_length_getter_keeps_range() {
    assert_eq!(
        eval_str(
            "var o = {}; \
             Object.defineProperty(o, 'length', { \
               configurable: true, \
               get: function () { return 2; }, \
               set: function (v) {} }); \
             var r = Array.prototype.fill.call(o, 'v', 0, 1); \
             [r[0] === 'v' ? 1 : 0, r[1] === undefined ? 1 : 0].join('|')"
        ),
        "1|1"
    );
}

/// 5 实参 pack 区 [0, 5) 全量覆盖镜像着色集：全部实参存活，结果与插入区
/// 逐位保持。
#[test]
fn splice_five_arg_pack_keeps_all_args() {
    assert_eq!(
        eval_str(&with_length_getter(
            "var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b', 'c'); \
             delete Number.prototype.length; \
             [r.length, r[0], r[1], r[2], savedLen].join('|')"
        )),
        "1||||3"
    );
}

/// getter 自读镜像（`Array === Array`）：内联入口重载自愈，修后保真——
/// 入口重载语义不动，恢复边界 gate 不影响入口侧镜像可见性。
#[test]
fn splice_getter_mirror_self_read_stays_true() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             Object.defineProperty(Number.prototype, 'length', { \
               configurable: true, \
               get: function () { return Array === Array && Object === Object ? 1 : 0; }, \
               set: function (v) { savedLen = v; } }); \
             var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        ),
        "1|2"
    );
}

/// getter 内嵌套用户 CALL（帧路径）：帧返回后的恢复边界同门控，帧路径
/// 结果与无嵌套时逐位一致。
#[test]
fn splice_getter_nested_call_keeps_result() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             function probe() { return 42; } \
             Object.defineProperty(Number.prototype, 'length', { \
               configurable: true, \
               get: function () { var x = probe(); return savedLen; }, \
               set: function (v) { savedLen = v; } }); \
             var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        ),
        "1|2"
    );
}

/// indexOf 的 fromIndex 落 pack 区 reg 1：arraylike 接收者 length getter
/// 开窗后 fromIndex 读值不得折算为 0。
#[test]
fn index_of_arraylike_length_getter_keeps_from_index() {
    assert_eq!(
        eval_str(
            "var savedLen = 3; \
             var a = { 0: 10, 1: 20, 2: 10 }; \
             Object.defineProperty(a, 'length', { \
               configurable: true, \
               get: function () { return savedLen; }, \
               set: function (v) { savedLen = v; } }); \
              String(Array.prototype.indexOf.call(a, 10, 1))"
        ),
        "2"
    );
}

/// reduce 的 init 落 pack 区 reg 2：arraylike length getter 开窗后
/// init 读值不得被镜像值（Object 内置函数）替换。
#[test]
fn reduce_arraylike_length_getter_keeps_init() {
    assert_eq!(
        eval_str(
            "var savedLen = 1; \
             var a = { 0: 12 }; \
             Object.defineProperty(a, 'length', { \
               configurable: true, \
               get: function () { return savedLen; }, \
               set: function (v) { savedLen = v; } }); \
              String(Array.prototype.reduce.call(a, function (x, y) { return x + y; }, 1))"
        ),
        "13"
    );
}

/// `.apply` 转发同形态：TailCall 链 pack_end 嵌套存还后实参区保持。
#[test]
fn splice_apply_forward_keeps_delete_count() {
    assert_eq!(
        eval_str(&with_length_getter(
            "var r = Array.prototype.splice.apply(5, [0, 1, 'a', 'b']); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        )),
        "1|2"
    );
}

/// 无用户内联窗回归：无窗时 pack 区无覆写风险点，结果保持 node 语义。
#[test]
fn splice_no_window_result_unchanged() {
    assert_eq!(
        eval_str(
            "var r = Array.prototype.splice.call(5, 0, 1, 'a', 'b'); \
                  [r.length, r[0], r[1]].join('|')"
        ),
        "0||"
    );
}

/// 1 实参 pack 区 [0, 1) 不与镜像着色集求交：reg 0 恒为结果槽，此形无风险。
#[test]
fn splice_one_arg_pack_stays_safe() {
    assert_eq!(
        eval_str(&with_length_getter(
            "var r = Array.prototype.splice.call(5, 0); \
             delete Number.prototype.length; \
             [r.length, savedLen].join('|')"
        )),
        "1|0"
    );
}
