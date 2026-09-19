//! 数组 length 虚拟属性的赋值侧回归：`a.length = v` 的两次数值强转、strict/sloppy
//! 可写位语义、部分截断、增长、异常原值传播，以及 push/pop/shift/unshift 与
//! `a[i] = v` 字节码路径对不可写 length 的行为。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 求值并取字符串结果内容（字符串值走 lookup_str，非字符串走 Display）。
fn eval_str(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse ok");
    let module = Compiler::new().compile(&program).expect("compile ok");
    let mut vm = Vm::new();
    let val = vm.run(&Arc::new(module)).expect("run ok");
    if val.is_string() {
        vm.lookup_str(val).unwrap_or_default()
    } else {
        format!("{val}")
    }
}

#[test]
fn set_length_truncates_and_grows() {
    assert_eq!(eval_str("var a=[1,2,3]; a.length=1; ''+a.length+':'+(2 in a)"), "1:false");
    assert_eq!(eval_str("var a=[1]; a.length=3; ''+a.length+':'+(1 in a)"), "3:false");
}

#[test]
fn set_length_non_writable_strict_throws_sloppy_silent() {
    // sloppy：静默 no-op，length 不变。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); a.length=1; ''+a.length"),
        "3"
    );
    // strict：TypeError，length 不变。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); (function(){'use strict';try{a.length=1;return 'no-throw';}catch(e){return e.name;}})()+':'+a.length"),
        "TypeError:3"
    );
    // 同值赋值同样失败：不可写数据描述符先于 ArraySetLength 返回 false。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); (function(){'use strict';try{a.length=3;return 'no-throw';}catch(e){return e.name;}})()+':'+a.length"),
        "TypeError:3"
    );
}

#[test]
fn set_length_frozen_strict_throws_sloppy_silent() {
    // 冻结数组的 length 同样不可写：sloppy 静默而非恒抛。
    assert_eq!(eval_str("var a=Object.freeze([1,2,3]); a.length=1; ''+a.length"), "3");
    assert_eq!(
        eval_str("var a=Object.freeze([1,2,3]); (function(){'use strict';try{a.length=1;return 'no-throw';}catch(e){return e.name;}})()+':'+a.length"),
        "TypeError:3"
    );
}

#[test]
fn set_length_shrink_partial_truncation_at_blocker() {
    // strict：阻挡索引 1，length 收敛到 2，index 2 起删除，并抛 TypeError。
    assert_eq!(
        eval_str("var a=[0,1,2,3,4]; Object.defineProperty(a,'1',{configurable:false}); var r; (function(){'use strict';try{a.length=1;r='no-throw';}catch(e){r=e.name;}})(); r+':'+a.length+':'+a.hasOwnProperty('1')+':'+a.hasOwnProperty('2')"),
        "TypeError:2:true:false"
    );
    // sloppy：同样部分截断，静默失败。
    assert_eq!(
        eval_str("var a=[0,1,2,3,4]; Object.defineProperty(a,'1',{configurable:false}); a.length=0; ''+a.length+':'+a.hasOwnProperty('1')+':'+a.hasOwnProperty('2')"),
        "2:true:false"
    );
}

#[test]
fn set_length_growth_ignores_extensibility() {
    // ArraySetLength 的增长不要求对象可扩展：preventExtensions 后仍成功。
    assert_eq!(
        eval_str("var a=[1]; Object.preventExtensions(a); a.length=3; ''+a.length+':'+(1 in a)+':'+(2 in a)"),
        "3:false:false"
    );
}

#[test]
fn set_length_invalid_values_throw_range_error() {
    assert_eq!(
        eval_str("var a=[1,2,3]; var r=[]; [1.5,-1,4294967296,NaN].forEach(function(v){try{a.length=v;r.push('no-throw');}catch(e){r.push(e.name);}}); r.join(',')"),
        "RangeError,RangeError,RangeError,RangeError"
    );
}

#[test]
fn set_length_entry_non_writable_skips_coercion() {
    // 入口即不可写：可写位判定先于 ArrayLength，两次强转不执行（valueOf 零调用），
    // sloppy 静默、strict 抛 TypeError 而非强转期抛出的自定义 kind。
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); var n=0; var v={valueOf:function(){n++;throw new RangeError('x');}}; a.length=v; n+':'+a.length"),
        "0:3"
    );
    assert_eq!(
        eval_str("var a=[1,2,3]; Object.defineProperty(a,'length',{writable:false}); var n=0; var v={valueOf:function(){n++;throw new RangeError('x');}}; var r; (function(){'use strict';try{a.length=v;r='no-throw';}catch(e){r=e.name;}})(); r+':'+n+':'+a.length"),
        "TypeError:0:3"
    );
}

#[test]
fn set_length_coerces_twice_before_writable_check() {
    // ToUint32 + ToNumber 各触发一次 ToPrimitive(number)；第二次强转收窄可写位后
    // 由可写位判定拒绝，且不做任何截断。
    assert_eq!(
        eval_str("var a=[1,2,3], n=0; var v={}; v[Symbol.toPrimitive]=function(h){n++;Object.defineProperty(a,'length',{writable:false});return 0;}; var r; (function(){'use strict';try{a.length=v;r='no-throw';}catch(e){r=e.name;}})(); r+':'+n+':'+a.length"),
        "TypeError:2:3"
    );
    // Reflect.set 以严格模式调用并投影为 false，两次强转同样执行。
    assert_eq!(
        eval_str("var a=[1,2,3], n=0; var v={}; v[Symbol.toPrimitive]=function(h){n++;Object.defineProperty(a,'length',{writable:false});return 0;}; Reflect.set(a,'length',v)+':'+n+':'+a.length"),
        "false:2:3"
    );
}

#[test]
fn set_length_coercion_exception_propagates_original_value() {
    // 强转期 valueOf / Symbol.toPrimitive 抛出的自定义错误须原值重抛（identity 保留）。
    assert_eq!(
        eval_str("var err=new Error('boom'); var v={valueOf:function(){throw err;}}; var a=[]; var c=null; try{a.length=v;}catch(e){c=e;} (c===err)+':'+c.name"),
        "true:Error"
    );
    assert_eq!(
        eval_str("var err=new RangeError('sym'); var v={}; v[Symbol.toPrimitive]=function(h){throw err;}; var a=[]; var c=null; try{a.length=v;}catch(e){c=e;} (c===err)+':'+c.name"),
        "true:RangeError"
    );
}

#[test]
fn set_length_bigint_and_symbol_throw_type_error() {
    assert_eq!(
        eval_str("var a=[]; var r=[]; try{a.length=1n;r.push('no-throw');}catch(e){r.push(e.name);} try{a.length=Symbol();r.push('no-throw');}catch(e){r.push(e.name);} r.join(',')"),
        "TypeError,TypeError"
    );
}

#[test]
fn set_elem_beyond_non_writable_length_respects_strict() {
    // `a[i] = v` 字节码路径：越界写须过 length 可写性检查，sloppy 静默、strict 抛。
    assert_eq!(
        eval_str(
            "var a=[]; Object.defineProperty(a,'length',{writable:false}); var i=3; a[i]=9; ''+a.length+':'+(3 in a)"
        ),
        "0:false"
    );
    assert_eq!(
        eval_str("var a=[]; Object.defineProperty(a,'length',{writable:false}); var i=3; var r; (function(){'use strict';try{a[i]=9;r='no-throw';}catch(e){r=e.name;}})(); r+':'+a.length+':'+(3 in a)"),
        "TypeError:0:false"
    );
}

#[test]
fn mutators_on_non_writable_length_throw() {
    // 空数组 + 不可写 length：四个变更方法收尾的 Set length 均抛 TypeError。
    assert_eq!(
        eval_str("var a=[]; Object.defineProperty(a,'length',{writable:false}); var r=[]; try{a.push();r.push('no-throw');}catch(e){r.push(e.name);} r.push(a.length); r.join(',')"),
        "TypeError,0"
    );
    assert_eq!(
        eval_str(
            "var a=[]; Object.defineProperty(a,'length',{writable:false}); try{a.pop();'no-throw';}catch(e){e.name}"
        ),
        "TypeError"
    );
    assert_eq!(
        eval_str(
            "var a=[]; Object.defineProperty(a,'length',{writable:false}); try{a.shift();'no-throw';}catch(e){e.name}"
        ),
        "TypeError"
    );
    assert_eq!(
        eval_str("var a=[]; Object.defineProperty(a,'length',{writable:false}); try{a.unshift();'no-throw';}catch(e){e.name}"),
        "TypeError"
    );
    // frozen 空数组同样四方法皆抛。
    assert_eq!(
        eval_str("var a=Object.freeze([]); var r=[]; try{a.push();r.push('no-throw');}catch(e){r.push(e.name);} try{a.pop();r.push('no-throw');}catch(e){r.push(e.name);} try{a.shift();r.push('no-throw');}catch(e){r.push(e.name);} try{a.unshift();r.push('no-throw');}catch(e){r.push(e.name);} r.join(',')"),
        "TypeError,TypeError,TypeError,TypeError"
    );
}

#[test]
fn push_element_write_uses_prototype_setter() {
    // 继承的索引 setter 在元素写入时执行并收窄 length，收尾 Set length 失败。
    assert_eq!(
        eval_str("var a=[]; var calls=0; Object.defineProperty(Array.prototype,'0',{set:function(v){Object.defineProperty(a,'length',{writable:false});calls++;}}); var r; try{a.push(1);r='no-throw';}catch(e){r=e.name;} r+':'+a.length+':'+a.hasOwnProperty(0)+':'+calls"),
        "TypeError:0:false:1"
    );
}

#[test]
fn set_elem_non_extensible_precedes_length_guard() {
    // 不可扩展 + length 不可写：extensible 检查先于 length 可写性判定，
    // 错误消息取 not extensible 形态（node 同序）。
    assert_eq!(
        eval_str("var a=[1]; Object.preventExtensions(a); Object.defineProperty(a,'length',{writable:false}); var m='none'; (function(){'use strict';try{a[5]=9;m='no-throw';}catch(e){m=String(e.message);}})(); /extensible/.test(m)+':'+a.length+':'+(5 in a)"),
        "true:1:false"
    );
    // 单条件各自独立生效：仅 length 不可写（可扩展）报 length 形态。
    assert_eq!(
        eval_str("var a=[1]; Object.defineProperty(a,'length',{writable:false}); var m='none'; (function(){'use strict';try{a[5]=9;m='no-throw';}catch(e){m=String(e.message);}})(); /writable/.test(m)+':'+a.length"),
        "true:1"
    );
}

#[test]
fn shift_getter_temporary_survives_setter_window() {
    // 首位为 hole：Get 落原型链 getter（返回新对象），循环内用户 setter 大量
    // 分配——getter 临时对象经结果寄存器钉为 GC 根，跨重入窗口存活，返回值
    // 身份保持。
    assert_eq!(
        eval_str("var box=null; var a=[,2,3]; Object.defineProperty(Array.prototype,'0',{get:function(){box={m:1};return box;},set:function(v){Object.defineProperty(this,0,{value:v,writable:true,enumerable:true,configurable:true});var t='z';for(var i=0;i<16;i++){t=t+t;}}}); var r=a.shift(); (r===box)+':'+r.m+':'+a.length+':'+a[0]+':'+a[1]"),
        "true:1:2:2:3"
    );
}

#[test]
fn mutators_still_work_with_writable_length() {
    // 重写后正常路径回归：push/pop/shift/unshift 的元素搬移与返回值。
    assert_eq!(eval_str("var a=[]; a.push(1,2); ''+a.length+':'+a[1]"), "2:2");
    assert_eq!(
        eval_str("var a=[1,2,3]; var p=a.pop(); var s=a.shift(); var u=a.unshift(9,8); p+':'+s+':'+u+':'+a.join(',')"),
        "3:1:3:9,8,2"
    );
}
