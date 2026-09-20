//! native 分支返回边界镜像重载的引擎钉：native 调用期间发生的全局 builtin
//! 写（A 侧存储 + 执行模块镜像槽同步），native 分支返回的窗口拷回把调用方
//! 镜像槽打回调用前值，返回边界的按 A 侧重载使外层裸读/typeof 与 A 侧一致。
//! 覆盖：`.call` 转发深度 ≥2（主形态）、深度 1 直调自愈、用户全局与 in/
//! globalThis 的 A 侧面、外层写自愈、帧路径对照、getter 内自读、splice 返回
//! 值面。期望值一律取 node v20.19.2 实测值。

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

// 共用的 length getter 装置：splice 的 arraylike length 走用户 accessor
// （带空 setter，保持属性可写形），getter 体在 native 调用期间执行对全局
// 的写，返回长度 1 使 splice 删一格。
// 装置前后半：getter 体拼入中间，直拼免转义（装置含 JS 花括号，不走 format 模板）。
const GETTER_HEAD: &str = "var o = {}; \
      Object.defineProperty(o, 'length', { configurable: true, \
        get: function () { ";
const GETTER_TAIL: &str = "; return 1; }, set: function () {} }); ";

/// 以指定 getter 体拼出「装置 + body 脚本」。
fn with_getter(getter_body: &str, body: &str) -> String {
    let mut s = String::from(GETTER_HEAD);
    s.push_str(getter_body);
    s.push_str(GETTER_TAIL);
    s.push_str(body);
    s
}

// ── `.call` 转发深度 ≥2：getter 写后外层镜像槽读 ──
#[test]
fn splice_call_getter_write_builtin_bare_read() {
    assert_eq!(
        eval_str(&with_getter("Array = 42", "Array.prototype.splice.call(o, 0, 1); String(Array === 42)")),
        "true"
    );
}

#[test]
fn splice_call_getter_write_builtin_typeof() {
    assert_eq!(
        eval_str(&with_getter("Array = 42", "Array.prototype.splice.call(o, 0, 1); typeof Array")),
        "number"
    );
}

#[test]
fn splice_call_getter_delete_builtin_typeof() {
    assert_eq!(
        eval_str(&with_getter("delete Array", "Array.prototype.splice.call(o, 0, 1); typeof Array")),
        "undefined"
    );
}

#[test]
fn splice_call_getter_write_builtin_pre_post_read() {
    assert_eq!(
        eval_str(
            "var pre = (Array === 42); \
             var o = {}; \
             Object.defineProperty(o, 'length', { configurable: true, \
               get: function () { Array = 42; return 1; }, set: function () {} }); \
             Array.prototype.splice.call(o, 0, 1); \
             pre + '|' + (Array === 42)"
        ),
        "false|true"
    );
}

// ── 深度 1 直调（无窗口保存边界）：返回后外层读自始与 A 侧一致 ──
#[test]
fn join_direct_call_getter_write_builtin_bare_read() {
    assert_eq!(
        eval_str(
            "var o = Object.create(Array.prototype); \
             Object.defineProperty(o, 'length', { configurable: true, \
               get: function () { Array = 42; return 1; }, set: function () {} }); \
             o.join(','); \
             String(Array === 42)"
        ),
        "true"
    );
}

#[test]
fn join_direct_call_getter_write_builtin_typeof() {
    assert_eq!(
        eval_str(
            "var o = Object.create(Array.prototype); \
             Object.defineProperty(o, 'length', { configurable: true, \
               get: function () { Array = 42; return 1; }, set: function () {} }); \
             o.join(','); \
             typeof Array"
        ),
        "number"
    );
}

// ── 用户全局无镜像槽：LOAD_GLOBAL 直读 A 侧，与 native 窗无关 ──
#[test]
fn splice_call_getter_write_user_global_bare_read() {
    assert_eq!(
        eval_str(&with_getter("u221 = 42", "Array.prototype.splice.call(o, 0, 1); String(u221 === 42)")),
        "true"
    );
}

#[test]
fn splice_call_getter_write_user_global_typeof() {
    assert_eq!(
        eval_str(&with_getter("u221 = 42", "Array.prototype.splice.call(o, 0, 1); typeof u221")),
        "number"
    );
}

// ── A 侧反射面：in / globalThis 直读 A 侧，镜像槽新旧不影响 ──
#[test]
fn splice_call_getter_write_builtin_in_check() {
    assert_eq!(
        eval_str(&with_getter(
            "Array = 42",
            "Array.prototype.splice.call(o, 0, 1); String('Array' in globalThis)"
        )),
        "true"
    );
}

#[test]
fn splice_call_getter_write_builtin_globalthis_reflect() {
    assert_eq!(
        eval_str(&with_getter(
            "Array = 42",
            "Array.prototype.splice.call(o, 0, 1); String(globalThis.Array === 42)"
        )),
        "true"
    );
}

// ── 外层写自愈：调用后外层写同步镜像 + A 侧 ──
#[test]
fn splice_call_getter_write_then_outer_write() {
    assert_eq!(
        eval_str(&with_getter(
            "Array = 42",
            "Array.prototype.splice.call(o, 0, 1); Array = 7; String(Array === 7)"
        )),
        "true"
    );
}

// ── 帧路径对照：帧恢复边界重载在位，结果与直读一致 ──
#[test]
fn getter_in_user_function_frame_path_bare_read() {
    assert_eq!(
        eval_str(
            "function f(o) { var L = o.length; return L; } \
             var o = {}; \
             Object.defineProperty(o, 'length', { configurable: true, \
               get: function () { Array = 42; return 1; }, set: function () {} }); \
             f(o); \
             String(Array === 42)"
        ),
        "true"
    );
}

// ── getter 内模块自读与返回值面：模块内槽同步在位，arraylike 协议值不受镜像影响 ──
#[test]
fn splice_call_getter_self_read_after_write() {
    assert_eq!(
        eval_str(
            "var o = {}; \
             Object.defineProperty(o, 'length', { configurable: true, \
               get: function () { Array = 42; return (Array === 42) ? 1 : 99; }, set: function () {} }); \
             var r = Array.prototype.splice.call(o, 0, 1); \
             String(r.length)"
        ),
        "1"
    );
}

#[test]
fn splice_call_getter_write_builtin_result_length() {
    assert_eq!(
        eval_str(&with_getter(
            "Array = 42",
            "var r = Array.prototype.splice.call(o, 0, 1); String(r.length)"
        )),
        "1"
    );
}
