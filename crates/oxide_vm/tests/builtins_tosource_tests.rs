//! toSource 遗留扩展（九原型 own 方法与 null/undefined this 门禁）单测。
//!
//! 验收面：九原型 own toSource 存在性、描述符非枚举、null/undefined/de-Reference
//! 三形态门禁抛 TypeError、返回值形态抽查。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

const CLASSES: [&str; 9] = ["Object", "Function", "Array", "String", "Boolean", "Number", "Date", "RegExp", "Error"];

// 九原型 own toSource 存在且为函数。
#[test]
fn tosource_own_exists_on_nine_prototypes() {
    let mut vm = Vm::new();
    for cls in CLASSES {
        let source = format!("typeof {}.prototype.toSource", cls);
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "function", "toSource missing on {cls}");
    }
}

// 九原型 toSource 描述符 { writable:true, enumerable:false, configurable:true }。
#[test]
fn tosource_descriptor_non_enumerable() {
    let mut vm = Vm::new();
    for cls in CLASSES {
        let source = format!(
            "(() => {{ var d = Object.getOwnPropertyDescriptor({}.prototype, 'toSource'); return d.writable + ',' + d.enumerable + ',' + d.configurable; }})()",
            cls
        );
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "true,false,true", "descriptor wrong on {cls}");
    }
}

// 九原型 toSource.call(null) 抛 TypeError（门禁核）。
#[test]
fn tosource_rejects_null_this() {
    let mut vm = Vm::new();
    for cls in CLASSES {
        let source = format!(
            "(() => {{ try {{ {}.prototype.toSource.call(null); return 'no-throw'; }} catch (e) {{ return e instanceof TypeError ? 'TypeError' : 'other'; }} }})()",
            cls
        );
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "TypeError", "null this not rejected on {cls}");
    }
}

// 九原型 toSource.call(undefined) 抛 TypeError（门禁核）。
#[test]
fn tosource_rejects_undefined_this() {
    let mut vm = Vm::new();
    for cls in CLASSES {
        let source = format!(
            "(() => {{ try {{ {}.prototype.toSource.call(undefined); return 'no-throw'; }} catch (e) {{ return e instanceof TypeError ? 'TypeError' : 'other'; }} }})()",
            cls
        );
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "TypeError", "undefined this not rejected on {cls}");
    }
}

// 九原型 (0, X.prototype.toSource)() de-Reference 形态抛 TypeError。
#[test]
fn tosource_rejects_dereference_call() {
    let mut vm = Vm::new();
    for cls in CLASSES {
        let source = format!(
            "(() => {{ try {{ (0, {}.prototype.toSource)(); return 'no-throw'; }} catch (e) {{ return e instanceof TypeError ? 'TypeError' : 'other'; }} }})()",
            cls
        );
        let result = eval(&mut vm, &source).unwrap();
        assert_eq!(to_str(&vm, result), "TypeError", "de-Reference not rejected on {cls}");
    }
}

// 返回值形态抽查：Object/Array/Boolean/Number/String 五个最小合理形。
#[test]
fn tosource_return_value_forms() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.prototype.toSource.call({})").unwrap();
    assert_eq!(to_str(&vm, result), "{}");

    let result = eval(&mut vm, "Array.prototype.toSource.call([])").unwrap();
    assert_eq!(to_str(&vm, result), "[]");

    let result = eval(&mut vm, "Boolean.prototype.toSource.call(true)").unwrap();
    assert_eq!(to_str(&vm, result), "true");

    let result = eval(&mut vm, "Number.prototype.toSource.call(42)").unwrap();
    assert_eq!(to_str(&vm, result), "42");

    let result = eval(&mut vm, "String.prototype.toSource.call('hi')").unwrap();
    assert_eq!(to_str(&vm, result), "\"hi\"");
}

// Function 臂：引擎无函数源文本，恒返空串。
#[test]
fn tosource_function_returns_empty() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Function.prototype.toSource.call(function(){})").unwrap();
    assert_eq!(to_str(&vm, result), "");
}

// RegExp 臂：/source/flags 斜杠形。
#[test]
fn tosource_regexp_slash_form() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "RegExp.prototype.toSource.call(/ab+c/i)").unwrap();
    assert_eq!(to_str(&vm, result), "/ab+c/i");
}

// Error 臂：new Error("message") 构造形。
#[test]
fn tosource_error_construction_form() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Error.prototype.toSource.call(new Error('boom'))").unwrap();
    assert_eq!(to_str(&vm, result), "new Error(\"boom\")");
}

// Date 臂：new Date(<ms>) 构造形。
#[test]
fn tosource_date_construction_form() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Date.prototype.toSource.call(new Date(1234))").unwrap();
    assert_eq!(to_str(&vm, result), "new Date(1234)");
}
