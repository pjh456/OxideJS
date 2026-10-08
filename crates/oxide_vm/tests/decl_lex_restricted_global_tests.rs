//! 脚本顶层 lexical 声明合法遮蔽三常量全局名（undefined/NaN/Infinity）：
//! let/const/class 声明名撞三常量合法（对齐 V8/node 现行行为；ES2025 规范
//! GDI lexNames 臂仍保留受限全局属性检查，属有意偏离，语料负面 oracle 已
//! 登记跳过集）。裸读读绑定槽、裸写/delete 不落全局对象，全局属性描述符
//! 不变；局部作用域（函数体/块/try）与 eval 代码为合法遮蔽不拒；三常数值
//! 使用不受影响。
//!
//! class 名撞 undefined/eval 不在本集（解析器 early-error 族独立拒绝，类体
//! 恒严格模式，与本集机制无关，见 top_level_class_undefined_rejected /
//! top_level_class_eval_name_rejected）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    vm.run(&Arc::new(module))
}

/// 断言编译 + 运行均成功，返回完成值。
fn assert_ok(source: &str) -> JsValue {
    eval(source).unwrap_or_else(|e| panic!("expected ok, got: {e}\nsource: {source}"))
}

/// 断言编译 + 运行均成功且完成值为字符串，返回字符串内容。
fn assert_ok_str(source: &str) -> String {
    let value = assert_ok(source);
    if value.is_string() {
        // SAFETY: 值已确认字符串类别，指针指向 VM 存活期内的 JsString。
        unsafe { &*value.as_string_ptr() }.as_str().to_string()
    } else {
        panic!("expected string, got {value:?}\nsource: {source}")
    }
}

// ── 脚本顶层 let/const/class 撞三常量：合法遮蔽 ──

#[test]
fn top_level_let_undefined_allowed() {
    // 独立形（脚本内无裸 undefined 引用，无镜像槽）：合法，完成值 undefined。
    let r = assert_ok("let undefined;");
    assert!(r.is_undefined(), "let undefined 完成值应为 undefined，实际 {r:?}");
}

#[test]
fn top_level_let_nan_allowed() {
    let r = assert_ok("let NaN;");
    assert!(r.is_undefined(), "let NaN 完成值应为 undefined，实际 {r:?}");
}

#[test]
fn top_level_let_infinity_allowed() {
    let r = assert_ok("let Infinity;");
    assert!(r.is_undefined(), "let Infinity 完成值应为 undefined，实际 {r:?}");
}

#[test]
fn top_level_const_nan_allowed() {
    let r = assert_ok("const NaN = 1; NaN");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_const_undefined_allowed() {
    let r = assert_ok("const undefined = 1; undefined");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_class_undefined_rejected() {
    // class 名撞 undefined 由解析器 early-error 族更早拒绝（SyntaxError 族，
    // 整程序拒绝可观察行为同形），与本集机制无关，断言接受任一拒绝相位。
    assert!(eval("class undefined {}").is_err(), "class undefined 类名应被拒绝");
}

#[test]
fn top_level_class_infinity_allowed() {
    // class 名同为 lexical 绑定：合法遮蔽，typeof 读绑定槽（类函数）。
    assert_eq!(assert_ok_str("class Infinity {} typeof Infinity"), "function");
}

#[test]
fn top_level_let_undefined_with_bare_use_allowed() {
    // 镜像形（脚本另有裸 undefined 引用）：声明点之后的裸读是词法绑定读，
    // 合法，完成值为绑定值 undefined。
    let r = assert_ok("let undefined; undefined");
    assert!(r.is_undefined(), "镜像形完成值应为 undefined，实际 {r:?}");
}

#[test]
fn top_level_let_destructured_nan_allowed() {
    // 解构叶子同属声明名，合法遮蔽。
    let r = assert_ok("let [NaN] = [1]; NaN");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_switch_case_let_nan_allowed() {
    // switch case 有独立 CaseBlock 作用域：case 内 lexical 是块局部绑定。
    let r = assert_ok("switch (0) { case 0: let NaN; } 1");
    assert_eq!(r.as_int(), 1);
}

// ── eval 门控：eval 代码声明实例化无受限检查 ──

#[test]
fn eval_script_let_nan_allowed() {
    // eval 代码在独立 lexical 环境实例化；外层裸读仍是三常量值
    //（NaN 自反性用 isNaN 断言，=== 对 NaN 恒 false）。
    let r = assert_ok("eval('let NaN;'); isNaN(NaN)");
    assert!(r.is_bool() && r.as_bool(), "eval 内 let NaN 应为合法遮蔽，实际 {r:?}");
}

// ── 值隔离与全局对象不变面：写/delete 落绑定槽，全局属性原值与描述符保留 ──

#[test]
fn top_level_let_undefined_value_isolated() {
    // 声明点后的裸读读绑定槽：得声明值，非全局属性值。
    let r = assert_ok("let undefined=5; undefined");
    assert_eq!(r.as_int(), 5);
}

#[test]
fn top_level_let_undefined_global_this_reflection() {
    // 全局对象属性原值保留：globalThis.undefined 仍是真 undefined（typeof 串形）。
    assert_eq!(assert_ok_str("let undefined=5; typeof globalThis.undefined"), "undefined");
}

#[test]
fn top_level_let_undefined_delete_not_global() {
    // 词法绑定 delete 返 false（DeleteBinding 非属性引用），全局属性原值保留。
    let r = assert_ok("let undefined=5; delete undefined");
    assert!(r.is_bool() && !r.as_bool(), "实际 {r:?}");
    let r = assert_ok("let undefined=5; delete undefined; undefined");
    assert_eq!(r.as_int(), 5);
}

#[test]
fn top_level_let_undefined_typeof() {
    // typeof 读绑定槽：声明值 5 的 typeof 为 number。
    assert_eq!(assert_ok_str("let undefined=5; typeof undefined"), "number");
}

#[test]
fn top_level_let_undefined_nested_capture() {
    // 嵌套函数读顶层词法绑定经 upvalue 捕获：得声明值。
    let r = assert_ok("let undefined=1; function f(){return undefined} f()");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_let_undefined_tdz_read_throws() {
    // 声明点前裸读是 TDZ 运行期 ReferenceError（错消息含 before initialization）。
    let err = eval("undefined; let undefined;").unwrap_err();
    assert!(err.contains("before initialization"), "got: {err}");
}

#[test]
fn top_level_let_undefined_descriptor_unchanged() {
    // 全局对象属性描述符不变：writable/configurable/enumerable 恒 false。
    let r = assert_ok(
        "let undefined=5; Object.getOwnPropertyDescriptor(globalThis,'undefined')
         .writable === false && Object.getOwnPropertyDescriptor(globalThis,'undefined')
         .configurable === false && Object.getOwnPropertyDescriptor(globalThis,'undefined')
         .enumerable === false",
    );
    assert!(r.is_bool() && r.as_bool(), "描述符应为只读不可配置不可枚举，实际 {r:?}");
}

// ── 假阳性守卫：合法面不得误拒 ──

#[test]
fn top_level_let_uses_nan_value_allowed() {
    // 三常量值使用（非声明名）不受检查影响（isNaN 断言，=== 对 NaN 恒 false）。
    let r = assert_ok("let x = NaN; isNaN(x)");
    assert!(r.is_bool() && r.as_bool(), "let x = NaN 应正常执行，实际 {r:?}");
}

#[test]
fn top_level_let_eval_name_allowed() {
    // eval 是全局对象可配置自有属性：合法遮蔽，不在受限集。
    let r = assert_ok("let eval; 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_const_eval_name_allowed() {
    let r = assert_ok("const eval = 1; 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_class_eval_name_rejected() {
    // eval 不在受限集（let/const 合法遮蔽），但 class 名撞 eval 由解析器
    // early-error 族拒绝：类体恒严格模式，类名绑定检查落在类作用域 strict
    // 判定内（V8 同形：sloppy `class eval {}` 亦 SyntaxError）。与本集
    // 机制无关，整程序拒绝可观察行为同形，断言任意拒绝相位。
    assert!(eval("class eval {}").is_err(), "class eval 类名应被拒绝");
}

#[test]
fn top_level_let_hasownproperty_allowed() {
    // hasOwnProperty 非全局对象自有属性（Object.prototype 继承）：合法遮蔽。
    let r = assert_ok("let hasOwnProperty; 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn block_level_let_nan_allowed() {
    // 顶层块内 lexical 声明是块局部绑定：不拒。
    let r = assert_ok("{ let NaN; } 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn try_block_let_nan_allowed() {
    // try 块 lexical 声明是 try 局部绑定：不拒。
    let r = assert_ok("try { let NaN; } catch (e) {} 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn function_body_let_nan_allowed() {
    // 函数体 lexical 声明是局部绑定：不拒。
    let r = assert_ok("function f(){ let NaN; return 1; } f() === 1");
    assert!(r.is_bool() && r.as_bool(), "函数体 let NaN 应为合法局部，实际 {r:?}");
}
