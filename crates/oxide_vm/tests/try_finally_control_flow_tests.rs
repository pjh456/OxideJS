//! try/finally + break/continue/return 控制流冒烟：finally 必须按语义执行。
//!
//! 覆盖：break/continue/return 穿越 finally、finally 内控制流覆盖、
//! 异常路径不回归、嵌套 try/finally、for-of 迭代器关闭、普通循环零开销路径。
//! 字符串值在此引擎 Display 为 `{string}`，断言一律用数值编码。

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

#[test]
fn break_runs_finally_and_exits_loop() {
    assert_eq!(
        eval("var fin = 0; var i = 0; for (; i < 5; ) { try { break; } finally { fin = 1; } i = i + 1; } fin * 10 + i"),
        "10"
    );
}

#[test]
fn continue_runs_finally_every_iteration() {
    assert_eq!(
        eval("var fin = 0; for (var i = 0; i < 5; i++) { try { continue; } finally { fin = fin + 1; } } fin"),
        "5"
    );
}

#[test]
fn return_runs_finally() {
    assert_eq!(
        eval("var fin = 0; function f() { try { return 1; } finally { fin = 1; } } var r = f(); r * 10 + fin"),
        "11"
    );
}

#[test]
fn break_inside_finally_breaks_loop() {
    assert_eq!(eval("var fin = 0; var n = 0; for (var i = 0; i < 5; i++) { try { } finally { fin = fin + 1; break; } n = n + 1; } fin * 10 + n"), "10");
}

#[test]
fn throw_still_runs_finally() {
    assert_eq!(
        eval("var fin = 0; function g() { try { throw 1; } finally { fin = 1; } } try { g(); } catch (e) {} fin"),
        "1"
    );
}

#[test]
fn nested_try_finally_both_run_on_break() {
    assert_eq!(eval("var log = 0; for (var i = 0; i < 3; i++) { try { try { break; } finally { log = log * 10 + 1; } } finally { log = log * 10 + 2; } } log"), "12");
}

#[test]
fn for_of_break_runs_finally() {
    assert_eq!(eval("var fin = 0; var it = 0; for (var x of [1,2,3]) { try { break; } finally { fin = 1; } it = it + 1; } fin * 10 + it"), "10");
}

#[test]
fn plain_loop_without_try_unchanged() {
    assert_eq!(eval("var s = 0; for (var i = 0; i < 5; i++) { s = s + i; } s"), "10");
}

#[test]
fn try_catch_break_does_not_leak_handler() {
    assert_eq!(
        eval("var caught = 0; for (var i = 0; i < 1; i++) { try { break; } catch (e) { caught = 1; } } try { throw 1; } catch (e) { caught = 2; } caught"),
        "2"
    );
}

#[test]
fn try_finally_break_does_not_leak_handler() {
    assert_eq!(
        eval("var caught = 0; for (var i = 0; i < 1; i++) { try { break; } finally { } } try { throw 1; } catch (e) { caught = 2; } caught"),
        "2"
    );
}

#[test]
fn return_in_finally_overrides() {
    assert_eq!(eval("function f() { try { return 1; } finally { return 2; } } f()"), "2");
}

#[test]
fn return_in_finally_overrides_exception() {
    assert_eq!(eval("function f() { try { throw 1; } finally { return 2; } } f()"), "2");
}

#[test]
fn break_in_finally_overrides_exception() {
    assert_eq!(
        eval("var n = 0; for (var i = 0; i < 5; i++) { try { try { throw 1; } finally { break; } } catch (e) { n = n + 1; } } n"),
        "0"
    );
}

#[test]
fn continue_in_finally_overrides_break() {
    assert_eq!(
        eval(
            "var log = 0; for (var i = 0; i < 3; i++) { try { break; } finally { log = log * 10 + i; continue; } } log"
        ),
        "12"
    );
}

#[test]
fn throw_in_finally_overrides_previous_exception() {
    assert_eq!(
        eval("var fin = 0; try { try { throw 1; } finally { throw 2; } } catch (e) { fin = e; } fin"),
        "2"
    );
}

#[test]
fn throw_then_finally_then_catch_chain_kept() {
    assert_eq!(
        eval("var fin = 0; var cc = 0; for (var i = 0; i < 3; i++) { try { try { throw i; } finally { fin = fin + 1; } } catch (e) { cc = cc + 1; } } fin * 10 + cc"),
        "33"
    );
}

#[test]
fn labeled_break_runs_finally() {
    assert_eq!(
        eval("var fin = 0; outer: for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { try { break outer; } finally { fin = fin + 1; } } } fin"),
        "1"
    );
}

#[test]
fn nested_function_break_ignores_caller_finally() {
    assert_eq!(
        eval("var log = 0; function inner() { for (var i = 0; i < 2; i++) { break; } return 1; } try { inner(); log = log * 10 + 1; } finally { log = log * 10 + 2; } log"),
        "12"
    );
}

#[test]
fn continue_with_finally_conditionally() {
    assert_eq!(
        eval("var fin = 0; var count = 0; for (var i = 0; i < 4; i++) { try { if (i % 2 === 0) continue; count = count + 1; } finally { fin = fin + 1; } } fin * 10 + count"),
        "42"
    );
}

#[test]
fn normal_finally_path_still_works() {
    assert_eq!(
        eval("var s = 0; for (var i = 0; i < 3; i++) { try { s = s + i; } finally { s = s + 10; } } s"),
        "33"
    );
}

#[test]
fn internal_loop_break_inside_try_does_not_trip_finally() {
    // break 目标是 try 域内的循环出口：不穿越 finally，finally 只走正常路径一次。
    assert_eq!(
        eval("var fin = 0; var i = 0; for (var k = 0; k < 3; k++) { try { while (i < 1) { break; } } finally { fin = fin + 1; } i = i + 1; } fin"),
        "3"
    );
}

#[test]
fn triple_nested_finally_all_run_on_break() {
    // break 穿越三层 finally，每个都执行一次。
    assert_eq!(
        eval("var log = 0; for (var i = 0; i < 3; i++) { try { try { try { break; } finally { log = log * 10 + 1; } } finally { log = log * 10 + 2; } } finally { log = log * 10 + 3; } } log"),
        "123"
    );
}

#[test]
fn return_crosses_multiple_finally() {
    assert_eq!(
        eval("var fin = 0; function f() { try { try { return 5; } finally { fin = fin + 1; } } finally { fin = fin + 10; } } var r = f(); r * 100 + fin"),
        "511"
    );
}

#[test]
fn continue_across_two_finally_runs_both_each_iteration() {
    // 内层 try/finally 包裹外层 try/finally，continue 逃出两者，两者每轮都执行。
    assert_eq!(
        eval("var a = 0; var b = 0; for (var i = 0; i < 3; i++) { try { try { continue; } finally { a = a + 1; } } finally { b = b + 1; } } a * 10 + b"),
        "33"
    );
}

#[test]
fn throw_in_finally_discards_pending_break() {
    // finally 内 throw 覆盖悬挂的 break 完成：break 不执行，异常向外传播。
    assert_eq!(
        eval("var fin = 0; var n = 0; for (var i = 0; i < 5; i++) { try { try { break; } finally { throw 9; } } catch (e) { n = e; } } n"),
        "9"
    );
}

#[test]
fn return_in_try_catch_cleans_handler() {
    assert_eq!(eval("function f(){ try{ return 1 } catch(e){ return 2 } }; f()"), "1");
}

#[test]
fn return_in_try_catch_value_path() {
    assert_eq!(eval("function f(){ try{ throw 1 } catch(e){ return e } }; f()"), "1");
}

#[test]
fn return_in_try_finally_runs_finally() {
    assert_eq!(
        eval("var fin = 0; function f(){ try{ return 1 } finally{ fin = 1 } } f() * 10 + fin"),
        "11"
    );
}

#[test]
fn finally_throw_overrides_return() {
    assert_eq!(eval("function f(){ try{ return 1 } finally { throw 2 } } ; try{ f() }catch(e){ e }"), "2");
}

#[test]
fn rethrow_in_leaked_catch_no_longer_hangs() {
    // return 穿过 try/catch 后，泄漏的 catch handler 不得在后续异常时被跳入：
    // 否则 catch 再 throw → 同一 handler 再弹 → 无限循环。修复后该 handler
    // 在 return 时被清理 / unwind 时被跳过，异常交给外层 catch。
    assert_eq!(
        eval("var n = 0; function f(){ try { return 1 } catch(e) { throw e; } } f(); try { undefined.x } catch(e){ n = 1; } n"),
        "1"
    );
}

#[test]
fn return_crosses_finally_below_outer_catch() {
    // return 穿过 finally（其下还有外层 catch）：finally 必须执行，外层 catch
    // 不得残留泄漏（该场景 TRY_END 无法从栈顶弹出，靠运行时扫描兜底）。
    assert_eq!(
        eval("var fin = 0; function f(){ try { try { return 1 } finally { fin = 1 } } catch(e){ return 2 } } f() * 10 + fin"),
        "11"
    );
}

#[test]
fn return_crosses_outer_finally_above_inner_catch() {
    assert_eq!(
        eval("var fin = 0; function f(){ try { try { return 1 } catch(e){ return 2 } } finally { fin = 1 } } f() * 10 + fin"),
        "11"
    );
}

#[test]
fn leaked_nested_handler_then_exception_is_caught() {
    assert_eq!(
        eval("function f(){ try { try { return 1 } finally { } } catch(e){ } } f(); var n = 0; try { undefined.x } catch(e){ n = (e.name === 'TypeError') ? 1 : 2 } n"),
        "1"
    );
}

// ── inline 同步回调 × 异常 unwind 回归 ──
// inline 调用（数组回调/replacer/getter）不 push 帧，callee 抛出的异常经
// dispatch_native_call 边界回到调用方 try/catch。以下断言保证：最小复现得
// 预期值、抛出的原始值不被包装成 Error 对象、嵌套/多级回调与 finally
// 组合的展开路径正确。

#[test]
fn inline_callback_throw_caught_by_caller_returns_expected() {
    assert_eq!(eval("try { [1].map(function(){ throw 1; }); } catch(e) { 42 }"), "42");
}

#[test]
fn inline_callback_throw_primitive_value_kept_original() {
    // throw 原始值经 inline 边界传播，catch 须收到原值而非包装的 Error 对象。
    assert_eq!(eval("var r = 0; try { [1].map(function(){ throw 2; }); } catch(e) { r = e; } r"), "2");
    assert_eq!(eval("var r = 0; try { [1].forEach(function(){ throw 's'; }); } catch(e) { r = (typeof e === 'string') ? 1 : 0; } r"), "1");
    assert_eq!(
        eval("var r = 0; try { [1].filter(function(){ throw null; }); } catch(e) { r = (e === null) ? 1 : 0; } r"),
        "1"
    );
    assert_eq!(
        eval(
            "var r = 0; try { [1].map(function(){ throw undefined; }); } catch(e) { r = (e === undefined) ? 1 : 0; } r"
        ),
        "1"
    );
    assert_eq!(
        eval("var r = 0; try { [1].map(function(){ throw false; }); } catch(e) { r = (e === false) ? 1 : 0; } r"),
        "1"
    );
}

#[test]
fn inline_callback_throw_error_object_kept_reference() {
    // Error 对象作为异常值传播时保持同一引用，catch 可读 message。
    assert_eq!(
        eval("try { [1].map(function(){ throw new Error('myerr'); }); } catch(e) { (e.message === 'myerr') ? 1 : 0 }"),
        "1"
    );
}

#[test]
fn nested_inline_callback_throw_caught_by_outer_catch() {
    assert_eq!(
        eval("var r = 0; try { [1].map(function(){ [1].map(function(){ throw 5; }); }); } catch(e) { r = e; } r"),
        "5"
    );
}

#[test]
fn multi_level_inline_chain_throw_caught_by_top_catch() {
    assert_eq!(
        eval("var r = 0; try { [1].map(function(){ [1].map(function(){ [1].map(function(){ throw 3; }); }); }); } catch(e) { r = e; } r"),
        "3"
    );
}

#[test]
fn inline_callback_inner_catch_normal_capture_continues() {
    assert_eq!(
        eval("var r = [1,2,3].map(function(x){ try { if (x == 2) throw 9; return x; } catch(e) { return 100; } }); r.length * 1000 + r[0] * 100 + r[1] * 10 + r[2]"),
        "4103"
    );
}
#[test]
fn inline_callback_inner_finally_runs_before_outer_catch() {
    // inline callee 内 finally 先执行，异常再传播给调用方 catch。
    assert_eq!(
        eval("var fin = 0; var r = 0; try { [1].map(function(){ try { throw 1; } finally { fin = 5; } }); } catch(e) { r = fin + 1; } r"),
        "6"
    );
}

#[test]
fn inline_callback_finally_throw_overrides_pending() {
    assert_eq!(
        eval("var r = 0; try { [1].map(function(){ try { throw 1; } finally { throw 2; } }); } catch(e) { r = e; } r"),
        "2"
    );
}

#[test]
fn inline_callback_throw_crosses_outer_finally_to_catch() {
    assert_eq!(
        eval("var log = []; try { try { [1].map(function(){ throw 1; }); } finally { log.push('f'); } } catch(e) { log.push('c'); } log.length"),
        "2"
    );
}

#[test]
fn inline_callback_throw_caught_by_inner_catch_rethrow_to_outer() {
    assert_eq!(
        eval("var r = 0; try { try { [1].map(function(){ throw 1; }); } catch(e) { r = 49; throw e; } } catch(e) { r = r * 10 + 50; } r"),
        "540"
    );
}

#[test]
fn inline_replace_replacer_throw_primitive_kept() {
    assert_eq!(
        eval("var r = 0; try { 'a'.replace(/a/, function(){ throw 8; }); } catch(e) { r = e; } r"),
        "8"
    );
}

#[test]
fn inline_getter_throw_primitive_kept() {
    assert_eq!(
        eval("var r = 0; var o = { get g() { throw 7; } }; try { var x = o.g; } catch(e) { r = e; } r"),
        "7"
    );
}

#[test]
fn inline_sort_comparator_throw_primitive_kept() {
    assert_eq!(
        eval("var r = 0; try { [2,1].sort(function(a,b){ [1].map(function(){ throw 13; }); return a - b; }); } catch(e) { r = e; } r"),
        "13"
    );
}

// ── finally 体直落进入回归（catch 体正常结束）──
// catch 体结束后指令直落入 finally 体（无 JMP），此前 finally_active 未被置位，
// finally 内 break/continue/return/throw 会触发"未进入"分支重入 finally 体、
// 副作用翻倍。专用入口标记统一置位后，以下断言得期望值（修复前为 buggy 值）。

#[test]
fn fallthrough_finally_return_runs_once() {
    assert_eq!(
        eval("var fin=0; function f(){ try { throw 1; } catch(e){} finally { fin=fin+1; return 2; } } var r=f(); r*10+fin"),
        "21"
    );
}

#[test]
fn fallthrough_finally_break_runs_once() {
    assert_eq!(
        eval("var fin=0; for(var i=0;i<2;i++){ try { throw 1; } catch(e){} finally { fin=fin+1; break; } } fin"),
        "1"
    );
}

#[test]
fn fallthrough_finally_continue_runs_once_per_iteration() {
    assert_eq!(
        eval("var fin=0; for(var i=0;i<5;i++){ try { throw 1; } catch(e){} finally { fin=fin+1; continue; } } fin"),
        "5"
    );
}

#[test]
fn fallthrough_finally_return_keeps_value() {
    assert_eq!(
        eval("var fin=0; function f(){ try { throw 1; } catch(e){} finally { fin++; return 7; } } var r=f(); r*10+fin"),
        "71"
    );
}

#[test]
fn fallthrough_finally_break_crosses_two_layers_once_each() {
    // 内层 finally 直落进入后 break outer 穿越两层，两层各执行一次不重复。
    assert_eq!(
        eval("var log=0; outer: for(var i=0;i<2;i++){ try { try { throw 1; } catch(e){} finally { log=log*10+1; break outer; } } finally { log=log*10+2; } } log"),
        "12"
    );
}

#[test]
fn fallthrough_finally_throw_does_not_reenter_finally() {
    // finally 内 throw：置位保证 unwind 判定"新异常在 finally 体内"不重入，
    // 异常交外层 catch，finally 只执行一次。
    assert_eq!(
        eval("var fin=0; function f(){ try { throw 1; } catch(e){} finally { fin=fin+1; throw 9; } } try { f(); } catch(e){ fin=fin*10+e; } fin"),
        "19"
    );
}

#[test]
fn fallthrough_finally_nested_finally_return_runs_once_each() {
    // finally 内嵌套 finally + return：两层各执行一次，return 值不受影响。
    assert_eq!(
        eval("var fin=0; function f(){ try { throw 1; } catch(e){} finally { try { } finally { fin++; } return 7; } } var r=f(); r*10+fin"),
        "71"
    );
}

#[test]
fn fallthrough_finally_normal_completion_guard() {
    // 直落本身非缺陷：无完成语义时 finally 只执行一次（守卫用例）。
    assert_eq!(eval("var fin=0; try { throw 1; } catch(e){} finally { fin=fin+1; } fin"), "1");
}
