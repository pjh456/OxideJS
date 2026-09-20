//! catch 参数捕获与每入口新 cell 形态锁：catch 参数绑定名（含解构模式全名）计入
//! 本函数 own 集，被嵌套闭包捕获时 catch 入口发 MAKE_CELL_FRESH——每次 catch 执行
//! 为参数新建词法环境（替换上一入口的 cell 而非原地覆写），子模块经 upvalue 读；
//! try 体/finally 体闭包对参数的域外引用保持运行期 ReferenceError。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 只暴露 number/bool，不暴露字符串内容），
//! 字符串比较在引擎内完成。

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

/// 断言运行期抛出未捕获 ReferenceError（域外引用守卫的期望形态）。
fn assert_uncaught_reference_error(source: &str, msg: &str) {
    let res = eval(source);
    assert!(
        res.starts_with("vm error") && res.contains("ReferenceError"),
        "{msg}: expected an uncaught ReferenceError, got: {res}"
    );
}

// ── 闭包捕获参数：直引 / 模式名 / 同名遮蔽 / 嵌套函数形 / 顶层 / 写穿 / 跨帧 ──

#[test]
fn catch_param_closure_direct_ref_reads_thrown_value() {
    // 主形态：catch 体闭包直引参数，读值为抛入的异常值。
    assert_eq!(
        eval("(() => { let f; try { throw 1; } catch (e) { f = () => e; } return f(); })()"),
        "1",
        "closure in catch body must read the thrown value through the parameter cell"
    );
}

#[test]
fn catch_param_destructured_name_captured_by_closure() {
    // 解构参数模式内的绑定名与裸参数名同口径：闭包捕获读属性值。
    assert_eq!(
        eval("(() => { let f; try { throw { message: 7 }; } catch ({ message: e }) { f = () => e; } return f(); })()"),
        "7",
        "destructuring catch parameter names are captured bindings too"
    );
}

#[test]
fn catch_param_shadows_outer_var_closure_reads_param() {
    // 外层 var 同名遮蔽：catch 体内的闭包捕获参数（内层绑定），读抛入值而非外层 var。
    assert_eq!(
        eval("(() => { var e = 99; try { throw 5; } catch (e) { const f = () => e; return f(); } })()"),
        "5",
        "closure captures the catch parameter, not the same-named outer var"
    );
}

#[test]
fn catch_param_shadows_function_param_closure_reads_param() {
    // 外层函数形参同名遮蔽：函数作用域内 catch 参数遮蔽形参，闭包读参数。
    assert_eq!(
        eval("(function(e) { try { throw 3; } catch (e) { const f = () => e; return f(); } })(88)"),
        "3",
        "closure captures the catch parameter, not the same-named outer function param"
    );
}

#[test]
fn catch_param_function_decl_inside_catch_reads_param() {
    // 函数声明形（非箭头）：catch 体内声明的函数引用参数，读抛入值。
    assert_eq!(
        eval("(() => { let f; try { throw 6; } catch (e) { function g() { return e; } f = g; } return f(); })()"),
        "6",
        "function declared in catch body reads the captured parameter"
    );
}

#[test]
fn catch_param_class_method_inside_catch_reads_param() {
    // 类方法形：catch 体内类表达式的方法引用参数，读抛入值。
    assert_eq!(
        eval(
            "(() => { let C; try { throw 8; } catch (e) { C = class { m() { return e; } }; } return new C().m(); })()"
        ),
        "8",
        "class method defined in catch body reads the captured parameter"
    );
}

#[test]
fn catch_param_top_level_closure_reads_param() {
    // 脚本顶层 catch 参数被顶层闭包捕获：顶层 cell 层同样承载参数 cell。
    assert_eq!(
        eval("let f; try { throw 11; } catch (e) { f = () => e; } f()"),
        "11",
        "top-level catch parameter is capturable by top-level closures"
    );
}

#[test]
fn catch_param_reassign_then_closure_reads_new_value() {
    // 写穿面：catch 体内重赋参数后闭包读新值（赋值须写穿共享单元）。
    assert_eq!(
        eval("(() => { let f; try { throw 1; } catch (e) { e = 99; f = () => e; } return f(); })()"),
        "99",
        "assignment to the catch parameter writes through the captured cell"
    );
}

#[test]
fn catch_param_recursive_frames_each_frame_isolated() {
    // 跨帧隔离：同名参数各调用帧独立 cell，闭包读各自帧的抛入值。
    assert_eq!(
        eval(
            "(() => { function g(n) { try { throw n; } catch (e) { return () => e; } } \
              let fs = [g(1), g(2)]; return fs.map(f => f()).join('|') === '1|2'; })()"
        ),
        "true",
        "each call frame builds its own parameter cell for the closure"
    );
}

#[test]
fn catch_param_default_value_inner_fn_reads_param() {
    // 参数默认值时序：模式默认值内嵌套函数引用参数，绑定先于默认值求值。
    assert_eq!(
        eval("(() => { let f; try { throw 4; } catch (e) { const g = (x = () => e) => x; f = g(); } return f(); })()"),
        "4",
        "nested function in a parameter default reads the catch parameter"
    );
}

// ── 每 catch 入口新 cell（替换而非覆写）──

#[test]
fn catch_param_loop_reentry_each_iteration_new_cell() {
    // 循环重入：每迭代 catch 入口新 cell，各迭代闭包读各自迭代值。
    assert_eq!(
        eval(
            "(() => { let r = []; for (let i = 0; i < 2; i++) { try { throw i; } catch (e) { r.push(() => e); } } \
              return r.map(f => f()).join('|') === '0|1'; })()"
        ),
        "true",
        "each loop iteration's catch entry builds a fresh parameter cell"
    );
}

#[test]
fn catch_param_consecutive_try_catch_each_entry_new_cell() {
    // 无循环连续两个 try/catch 同名参数：第二个入口新 cell 不覆写第一个入口闭包
    // 已持的 cell 指针（门控若按循环在飞判定，本形态读错值）。
    assert_eq!(
        eval(
            "(() => { let r = []; try { throw 1; } catch (e) { r.push(() => e); } \
              try { throw 2; } catch (e) { r.push(() => e); } \
              return r.map(f => f()).join('|') === '1|2'; })()"
        ),
        "true",
        "consecutive catch entries with the same parameter name build fresh cells"
    );
}

#[test]
fn catch_param_nested_catch_outer_closure_keeps_old_cell() {
    // 同名嵌套 catch：内层入口新 cell 替换槽位，外层闭包仍持旧 cell 指针读旧值。
    assert_eq!(
        eval(
            "(() => { let outer; try { throw 1; } catch (e) { outer = () => e; \
              try { throw 2; } catch (e) { } } return outer(); })()"
        ),
        "1",
        "inner catch entry must not clobber the outer closure's cell"
    );
}

// ── 守卫：既有直读/遮蔽/B022 模式面语义保真 ──

#[test]
fn catch_param_direct_read_in_body() {
    // 无闭包直读参数（最简形态）：直读保真。
    assert_eq!(
        eval("try { throw 42; } catch (e) { e }"),
        "42",
        "direct read of the catch parameter in its own body"
    );
}

#[test]
fn catch_param_shadows_outer_var_direct_read() {
    // var 同名遮蔽直读（无闭包）：catch 体内直读参数值。
    assert_eq!(
        eval("(() => { var e = 99; try { throw 5; } catch (e) { return e; } })()"),
        "5",
        "direct read in catch body sees the parameter, not the outer var"
    );
}

#[test]
fn catch_param_destructured_direct_read() {
    // 解构参数直读（无闭包）：模式绑定求值后直读求和。
    assert_eq!(
        eval("(() => { try { throw [1, 2]; } catch ([a, b]) { return a + b; } })()"),
        "3",
        "destructuring catch parameter direct read"
    );
}

#[test]
fn catch_param_pattern_default_value_evaluated() {
    // 模式默认值面：抛入值非 undefined 时默认值不求值，直读抛入元素。
    assert_eq!(
        eval("(() => { const g = () => 55; try { throw [1]; } catch ([x = g()]) { return x; } })()"),
        "1",
        "pattern default value is not evaluated when the thrown element is present"
    );
}

#[test]
fn catch_param_ref_from_try_body_stays_reference_error() {
    // 域外引用：try 体闭包引用 catch 参数——参数在 try 体域内未声明，
    // 修复后仍须运行期 ReferenceError（与 node 一致）。
    assert_uncaught_reference_error(
        "(() => { let f; try { f = () => e; throw 1; } catch (e) {} return f(); })()",
        "closure in try body referencing the catch parameter must stay unresolvable",
    );
}

#[test]
fn catch_param_ref_from_finally_body_stays_reference_error() {
    // 域外引用：finally 体闭包引用 catch 参数——参数作用域不含 finally 体，
    // 修复后仍须运行期 ReferenceError（与 node 一致）。
    assert_uncaught_reference_error(
        "(() => { let f; try { throw 1; } catch (e) {} finally { f = () => e; } return f(); })()",
        "closure in finally body referencing the catch parameter must stay unresolvable",
    );
}
