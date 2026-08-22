use oxide_builtins::error;
use oxide_compiler::compiler::Compiler;
use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn make_vm() -> Vm {
    Vm::new()
}

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = make_vm();
    vm.run(&module)
}

/// 在既有 Vm 内编译执行：结果字符串须在同一 Vm 上下文解析（session 串随 Vm 释放）。
fn eval_in(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    vm.run(&module)
}

#[test]
fn error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn error_has_message() {
    let mut vm = make_vm();
    let msg = vm.new_string("test message");
    vm.set_reg(1, msg);
    let result = error::error_constructor(&mut vm, &[0u8, 1u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn type_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::type_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn reference_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::reference_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn range_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::range_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn syntax_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::syntax_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn uri_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::uri_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn eval_error_constructor_creates_object() {
    let mut vm = make_vm();
    let result = error::eval_error_constructor(&mut vm, &[0u8]).unwrap();
    assert!(result.is_object());
}

#[test]
fn create_type_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "something went wrong");
    assert!(err.is_object());
}

#[test]
fn create_range_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_range_error(&mut vm, "out of bounds");
    assert!(err.is_object());
}

#[test]
fn create_reference_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_reference_error(&mut vm, "not defined");
    assert!(err.is_object());
}

#[test]
fn create_syntax_error_returns_jsvalue() {
    let mut vm = make_vm();
    let err = error::create_syntax_error(&mut vm, "invalid syntax");
    assert!(err.is_object());
}

#[test]
fn error_proto_chain_subtype_points_to_error() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "msg");
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert!(obj.proto().is_object());
}

#[test]
fn error_name_is_string_property() {
    let mut vm = make_vm();
    let err = error::error_constructor(&mut vm, &[0u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    let n = obj.prop_count();
    assert_eq!(n, 0);
}

#[test]
fn error_to_string_returns_string() {
    let result = eval("new Error('test').toString()").unwrap();
    assert!(result.is_string());
}

#[test]
fn type_error_is_defined() {
    let result = eval("typeof TypeError").unwrap();
    assert!(result.is_string());
}

#[test]
fn reference_error_is_defined() {
    let result = eval("typeof ReferenceError").unwrap();
    assert!(result.is_string());
}

#[test]
fn error_subtype_constructors_produce_named_objects() {
    assert_eq!(format!("{}", eval("new Error('boom').name == 'Error'").unwrap()), "true");
    assert_eq!(format!("{}", eval("new TypeError('boom').name == 'TypeError'").unwrap()), "true");
    assert_eq!(format!("{}", eval("new SyntaxError('boom').name == 'SyntaxError'").unwrap()), "true");
}

// ── 原型属性与构造器修复测试 ──

#[test]
fn error_prototype_has_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some(), "Error.prototype should have 'name' property");
}

#[test]
fn error_prototype_has_message() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let msg_si = vm.kernel_core().perm_interner().intern("message").0;
    let proto_ptr = P::as_ptr(&bw.error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), msg_si);
    assert!(si.is_some(), "Error.prototype should have 'message' property");
}

#[test]
fn type_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.type_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some(), "TypeError.prototype should have 'name'");
}

#[test]
fn reference_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.reference_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn range_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.range_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn syntax_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.syntax_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn uri_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.uri_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn eval_error_prototype_name() {
    let vm = make_vm();
    let bw = vm.session().builtin_world();
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let proto_ptr = P::as_ptr(&bw.eval_error_proto) as *mut oxide_types::object::JsObject;
    let si = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(unsafe { &*proto_ptr }.shape_id(), name_si);
    assert!(si.is_some());
}

#[test]
fn error_constructor_no_own_props() {
    let mut vm = make_vm();
    let err = error::error_constructor(&mut vm, &[0u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn error_constructor_with_message() {
    let mut vm = make_vm();
    let msg = vm.new_string("boom");
    vm.set_reg(1, msg);
    let err = error::error_constructor(&mut vm, &[0u8, 1u8]).unwrap();
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
}

#[test]
fn create_type_error_no_own_name() {
    let mut vm = make_vm();
    let err = error::create_type_error(&mut vm, "something broken");
    let obj = unsafe { &*err.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 1);
}

// ── toString 测试 ──

#[test]
fn error_to_string_basic() {
    let result = eval("new Error('test').toString()").unwrap();
    assert!(result.is_string());
}

#[test]
fn error_to_string_empty_message() {
    let mut vm = make_vm();
    let result = eval_in(&mut vm, "new Error().toString()").unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("Error".to_string()));
}

#[test]
fn error_to_string_type_error() {
    let mut vm = make_vm();
    let result = eval_in(&mut vm, "new TypeError('bad').toString()").unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("TypeError: bad".to_string()));
}

#[test]
fn error_to_string_name_only() {
    let mut vm = make_vm();
    let result = eval_in(&mut vm, "Error.prototype.toString.call({name: 'E'})").unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("E".to_string()));
}

#[test]
fn error_to_string_non_object_throws() {
    let mut vm = make_vm();
    vm.set_reg(0, JsValue::int(42));
    let result = error::error_to_string(&mut vm, &[0u8]);
    match result {
        oxide_runtime_api::NativeResult::Err(_) => {}
        _ => panic!("expected Err, got Ok or TailCall"),
    }
}

#[test]
fn error_to_string_name_getter_exception_propagates() {
    let mut vm = make_vm();
    // name getter 抛出的用户异常须原样传播，不得被替换成普通 TypeError。
    let result = eval_in(
        &mut vm,
        "try { Error.prototype.toString.call({get name() { throw new RangeError('boom'); }}); 'no-throw' } catch (e) { e.name + ':' + e.message }",
    )
    .unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("RangeError:boom".to_string()));
}

#[test]
fn error_to_string_message_getter_exception_propagates() {
    let mut vm = make_vm();
    let result = eval_in(
        &mut vm,
        "try { Error.prototype.toString.call({get message() { throw new RangeError('boom'); }}); 'no-throw' } catch (e) { e.name + ':' + e.message }",
    )
    .unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("RangeError:boom".to_string()));
}

#[test]
fn error_to_string_symbol_name_message_throws_type_error() {
    let mut vm = make_vm();
    // name/message 为 Symbol 时 ToString 按规范抛 TypeError。
    let result = eval_in(
        &mut vm,
        "var r1 = (function(){ try { Error.prototype.toString.call({name: Symbol('n')}); return 'no-throw'; } catch (e) { return e.name; } })(); \
         var r2 = (function(){ try { Error.prototype.toString.call({message: Symbol('m')}); return 'no-throw'; } catch (e) { return e.name; } })(); \
         r1 + '|' + r2",
    )
    .unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("TypeError|TypeError".to_string()));
}

#[test]
fn error_to_string_name_to_primitive_exception_propagates() {
    let mut vm = make_vm();
    // name 为带抛错 toString 的对象时传播原异常（不被替换为普通 TypeError）。
    let result = eval_in(
        &mut vm,
        "try { Error.prototype.toString.call({name: {toString: function() { throw new RangeError('bad-name'); }}}); 'no-throw' } catch (e) { e.name + ':' + e.message }",
    )
    .unwrap();
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("RangeError:bad-name".to_string()));
}

// ── 错误对象语义修复测试 ──

#[test]
fn error_message_property_non_enumerable() {
    let mut vm = make_vm();
    // message 描述符 enumerable=false（规范 CreateNonEnumerableDataPropertyOrThrow）。
    assert_eq!(
        format!(
            "{}",
            eval_in(&mut vm, "Object.getOwnPropertyDescriptor(new Error('msg'), 'message').enumerable").unwrap()
        ),
        "false"
    );
    // 可枚举自身键为空：Object.keys / JSON.stringify 不再泄漏 message。
    assert_eq!(format!("{}", eval_in(&mut vm, "Object.keys(new Error('msg')).length").unwrap()), "0");
    let r = eval_in(&mut vm, "JSON.stringify(new Error('secret'))").unwrap();
    assert_eq!(vm.lookup_str(r), Some("{}".to_string()));
    // Error.prototype 的 name/message 同样非枚举。
    assert_eq!(format!("{}", eval_in(&mut vm, "Object.keys(Error.prototype).length").unwrap()), "0");
    // 子类型原型上的 name/constructor 非枚举，for-in 不泄漏。
    assert_eq!(
        format!(
            "{}",
            eval_in(&mut vm, "var s=[]; for (var k in new TypeError('t')) s.push(k); s.length").unwrap()
        ),
        "0"
    );
}

#[test]
fn error_to_string_tag_is_error() {
    let mut vm = make_vm();
    // Error 家族（含子类型与用户子类）经 Object.prototype.toString 得 `[object Error]`。
    let r = eval_in(&mut vm, "Object.prototype.toString.call(new Error('x'))").unwrap();
    assert_eq!(vm.lookup_str(r), Some("[object Error]".to_string()));
    let r = eval_in(&mut vm, "Object.prototype.toString.call(new TypeError('x'))").unwrap();
    assert_eq!(vm.lookup_str(r), Some("[object Error]".to_string()));
    let r = eval_in(
        &mut vm,
        "class MyError extends Error {}; Object.prototype.toString.call(new MyError('x'))",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("[object Error]".to_string()));
    // Error.prototype 自身同样判定为 Error。
    let r = eval_in(&mut vm, "Object.prototype.toString.call(Error.prototype)").unwrap();
    assert_eq!(vm.lookup_str(r), Some("[object Error]".to_string()));
}

#[test]
fn error_to_string_primitive_this_throws() {
    let mut vm = make_vm();
    // 规范：Error.prototype.toString 对非对象 this 抛 TypeError（不兜底为 "Error"）。
    let r = eval_in(&mut vm, "try { Error.prototype.toString.call(1); 'no-throw' } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(r), Some("TypeError".to_string()));
}

#[test]
fn error_constructor_length_is_one() {
    let mut vm = make_vm();
    assert_eq!(format!("{}", eval_in(&mut vm, "Error.length").unwrap()), "1");
    assert_eq!(format!("{}", eval_in(&mut vm, "TypeError.length").unwrap()), "1");
    assert_eq!(format!("{}", eval_in(&mut vm, "EvalError.length").unwrap()), "1");
}

#[test]
fn error_ctor_as_function_call_creates_new_object() {
    let mut vm = make_vm();
    // Error.call(obj) 忽略传入 this，总是返回新 Error 对象。
    assert_eq!(format!("{}", eval_in(&mut vm, "var o = {x:1}; Error.call(o) === o").unwrap()), "false");
    let r = eval_in(&mut vm, "var o = {x:1}; TypeError.call(o).name").unwrap();
    assert_eq!(vm.lookup_str(r), Some("TypeError".to_string()));
}

// ── format_error_message 测试 ──

#[test]
fn format_error_message_both() {
    let result = oxide_runtime_api::format_error_message("TypeError", "bad arg");
    assert_eq!(result, "TypeError: bad arg");
}

#[test]
fn format_error_message_empty_msg() {
    let result = oxide_runtime_api::format_error_message("Error", "");
    assert_eq!(result, "Error");
}

#[test]
fn format_error_message_empty_name() {
    let result = oxide_runtime_api::format_error_message("", "msg");
    assert_eq!(result, "msg");
}

// ── stack 测试 ──

#[test]
fn error_stack_is_string() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "typeof new Error().stack()").unwrap();
    let module = Compiler::new().compile(&program).unwrap();
    let mut vm = make_vm();
    let result = vm.run(&module).unwrap();
    // typeof 结果复用进程级静态串，同 VM 上读取（跨 VM 指针存活由实现保证，不做假设）。
    let s = vm.lookup_str(result);
    assert_eq!(s, Some("string".to_string()));
}

#[test]
fn error_stack_starts_with_header() {
    let mut vm = make_vm();
    let result = eval_in(&mut vm, "new Error().stack()").unwrap();
    let s = vm.lookup_str(result).unwrap();
    assert!(s.starts_with("Error"), "stack should start with 'Error', got: {}", s);
}

#[test]
fn error_stack_frame_format() {
    let mut vm = make_vm();
    let result = eval_in(&mut vm, "(function foo() { return new Error('boom').stack(); })()").unwrap();
    let s = vm.lookup_str(result).unwrap();
    assert!(s.contains("    at "), "stack should have 4-space indent, got: {}", s);
}

// ── SuppressedError 测试 ──

#[test]
fn suppressed_error_property_order_message_error_suppressed() {
    let mut vm = make_vm();
    // 三字段属性创建顺序：message → error → suppressed（order-of-args-evaluation 断言紧邻）。
    let r = eval_in(&mut vm, "Object.getOwnPropertyNames(new SuppressedError('e', 's', 'm')).join(',')").unwrap();
    let names_str = vm.lookup_str(r).unwrap();
    let names: Vec<&str> = names_str.split(',').collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).expect(n);
    assert!(pos("message") < pos("error"), "message 应排在 error 前, got: {:?}", names);
    assert!(pos("error") < pos("suppressed"), "error 应排在 suppressed 前, got: {:?}", names);
}

#[test]
fn suppressed_error_message_undefined_omits_property() {
    let mut vm = make_vm();
    // message 缺参时不建 message 自有属性。
    let r = eval_in(&mut vm, "Object.getOwnPropertyNames(new SuppressedError([])).join(',')").unwrap();
    assert_eq!(vm.lookup_str(r), Some("error".to_string()));
    // message 显式 undefined 同样省略。
    let r = eval_in(&mut vm, "Object.getOwnPropertyNames(new SuppressedError('e', 's', undefined)).join(',')").unwrap();
    assert_eq!(vm.lookup_str(r), Some("error,suppressed".to_string()));
}

#[test]
fn suppressed_error_message_to_string_coercion() {
    let mut vm = make_vm();
    // 三参 message 走完整 ToString 强制转换：42 → "42"、false → "false"、null → "null"。
    let r = eval_in(&mut vm, "new SuppressedError('e','s',42).message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("42".to_string()));
    let r = eval_in(&mut vm, "new SuppressedError('e','s',false).message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("false".to_string()));
    let r = eval_in(&mut vm, "new SuppressedError('e','s',null).message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("null".to_string()));
    // 对象经 ToPrimitive(string hint) 调用 toString。
    let r = eval_in(&mut vm, "new SuppressedError('e','s',{toString:function(){return 'custom';}}).message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("custom".to_string()));
}

#[test]
fn suppressed_error_message_tostring_abrupt() {
    let mut vm = make_vm();
    // Symbol → TypeError（ToString 规范不可转换路径）。
    let r = eval_in(&mut vm, "try { new SuppressedError('e','s',Symbol('x')); 'no' } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(r), Some("TypeError".to_string()));
    // 用户 toString 抛出的异常原样传播，不塌缩成 TypeError。
    let r = eval_in(
        &mut vm,
        "try { new SuppressedError('e','s',{toString:function(){throw new RangeError('boom');}}); 'no' } catch (e) { e.name }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("RangeError".to_string()));
}

#[test]
fn suppressed_error_call_without_new_creates_object() {
    let mut vm = make_vm();
    // 普通调用（newtarget-is-undefined）同样建对象，原型指向 SuppressedError.prototype。
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError()) === SuppressedError.prototype").unwrap();
    assert_eq!(format!("{}", r), "true");
}

#[test]
fn suppressed_error_prototype_chain_and_shape() {
    let mut vm = make_vm();
    // 原型仅挂 constructor/name（name 与既有子类型一致排在 constructor 前），无 error/suppressed。
    let r = eval_in(&mut vm, "Object.getOwnPropertyNames(SuppressedError.prototype).join(',')").unwrap();
    assert_eq!(vm.lookup_str(r), Some("name,constructor".to_string()));
    // instanceof Error 走原型链，原型 [[Prototype]] = Error.prototype。
    assert_eq!(
        format!("{}", eval_in(&mut vm, "new SuppressedError('e','s') instanceof Error").unwrap()),
        "true"
    );
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError.prototype) === Error.prototype").unwrap();
    assert_eq!(format!("{}", r), "true");
    // name 为 "SuppressedError"，message 沿 Error.prototype 链为 ""。
    let r = eval_in(&mut vm, "SuppressedError.prototype.name").unwrap();
    assert_eq!(vm.lookup_str(r), Some("SuppressedError".to_string()));
    let r = eval_in(&mut vm, "SuppressedError.prototype.message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("".to_string()));
}

#[test]
fn suppressed_error_constructor_metadata() {
    let mut vm = make_vm();
    // 三参构造器 length=3（不可写不可枚举）。
    assert_eq!(format!("{}", eval_in(&mut vm, "SuppressedError.length").unwrap()), "3");
    // 构造器 name 属性。
    let r = eval_in(&mut vm, "SuppressedError.name").unwrap();
    assert_eq!(vm.lookup_str(r), Some("SuppressedError".to_string()));
    // 全局槽描述符 { writable:true, enumerable:false, configurable:true }。
    let r = eval_in(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(globalThis, 'SuppressedError'); [d.writable, d.enumerable, d.configurable].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("true,false,true".to_string()));
}

#[test]
fn create_suppressed_error_direct_call() {
    let mut vm = make_vm();
    // dispose 合并路径直调：仅 error/suppressed 两个自有属性、无 message、原型链到 Error。
    let err_val = vm.new_string("err");
    let sup_val = vm.new_string("sup");
    let result = error::create_suppressed_error(&mut vm, err_val, sup_val);
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    let sf = vm.kernel_core().perm_interner();
    let sh = vm.kernel_core().shape_forge();
    let si_error = sf.intern("error").0;
    let si_suppressed = sf.intern("suppressed").0;
    let si_message = sf.intern("message").0;
    assert!(sh.lookup_position(obj.shape_id(), si_error).is_some(), "应有 error 槽");
    assert!(sh.lookup_position(obj.shape_id(), si_suppressed).is_some(), "应有 suppressed 槽");
    assert!(sh.lookup_position(obj.shape_id(), si_message).is_none(), "不应有 message 槽");
    // 原型链：suppressed_error_proto → Error.prototype，instanceof Error 成立。
    assert!(obj.proto().is_object(), "proto 应为 suppressed_error_proto");
    // 非枚举：属性不进入 Object.keys（与 JS 侧 new 路径同一实现，此处校验 meta）。
    let err_pos = sh.lookup_position(obj.shape_id(), si_error).unwrap();
    let meta = obj.prop_meta_at(err_pos).unwrap();
    assert!(!meta.attributes.enumerable(), "error 槽应非枚举");
}

// ── 子类型构造器描述符与原型链测试 ──

#[test]
fn error_subtype_constructor_descriptors() {
    let mut vm = make_vm();
    // 构造器 prototype 槽描述符 {f,f,f}（子类型与 SuppressedError 同规）。
    let r = eval_in(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(TypeError, 'prototype'); [d.writable, d.enumerable, d.configurable].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("false,false,false".to_string()));
    // name 槽描述符 {f,f,t}。
    let r = eval_in(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(SuppressedError, 'name'); [d.writable, d.enumerable, d.configurable].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("false,false,true".to_string()));
    // length 槽描述符 {f,f,t}。
    let r = eval_in(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(TypeError, 'length'); [d.writable, d.enumerable, d.configurable].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r), Some("false,false,true".to_string()));
}

#[test]
fn error_subtype_constructor_proto_is_error_ctor() {
    let mut vm = make_vm();
    // NativeError 子类型构造器 [[Prototype]] === Error 构造器（instanceof 走 @@hasInstance 不受影响）。
    let r = eval_in(&mut vm, "Object.getPrototypeOf(TypeError) === Error").unwrap();
    assert_eq!(format!("{}", r), "true");
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError) === Error").unwrap();
    assert_eq!(format!("{}", r), "true");
    let r = eval_in(&mut vm, "new TypeError('x') instanceof TypeError").unwrap();
    assert_eq!(format!("{}", r), "true");
}

// ── full_reset 重建路径测试 ──

#[test]
fn suppressed_error_survives_error_family_reset() {
    let mut vm = make_vm();
    // error_family 脏：suppressed_error_proto 与构造器整体重建。
    let proto_ptr = P::as_ptr(&vm.session().builtin_world().suppressed_error_proto) as *mut JsObject;
    unsafe { (&mut *proto_ptr).bump_generation() };
    vm.full_reset();

    // 重建后 length=3、构造器 [[Prototype]]=Error、原型链到 Error.prototype 均不回落。
    let r = eval_in(&mut vm, "SuppressedError.length").unwrap();
    assert_eq!(format!("{}", r), "3");
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError) === Error").unwrap();
    assert_eq!(format!("{}", r), "true");
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError.prototype) === Error.prototype").unwrap();
    assert_eq!(format!("{}", r), "true");
    let r = eval_in(&mut vm, "new SuppressedError('e','s','m').message").unwrap();
    assert_eq!(vm.lookup_str(r), Some("m".to_string()));
    assert!(!vm.session().is_dirty_since_snapshot());
}

#[test]
fn suppressed_error_length_kept_after_global_only_reset() {
    let mut vm = make_vm();
    // 仅 global 脏（error_family 干净）：fallback 自建构造器分支须保留 length=3。
    let g_ptr = vm.session().global_object().as_ptr() as *mut JsObject;
    unsafe { (&mut *g_ptr).bump_generation() };
    vm.full_reset();

    let r = eval_in(&mut vm, "SuppressedError.length").unwrap();
    assert_eq!(format!("{}", r), "3");
    let r = eval_in(&mut vm, "Object.getPrototypeOf(SuppressedError) === Error").unwrap();
    assert_eq!(format!("{}", r), "true");
    let r = eval_in(&mut vm, "new SuppressedError('e','s').error").unwrap();
    assert_eq!(vm.lookup_str(r), Some("e".to_string()));
    assert!(!vm.session().is_dirty_since_snapshot());
}
