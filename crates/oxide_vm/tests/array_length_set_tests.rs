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

#[test]
fn new_array_numeric_length_builds_holes() {
    // 数值长度构造器建 n 个空洞：in / Object.keys / for-in / join /
    // JSON.stringify 全部按缺失语义观察。
    assert_eq!(eval_str("''+(0 in new Array(1))+':'+(1 in new Array(2))"), "false:false");
    assert_eq!(eval_str("Object.keys(new Array(3)).join(',')"), "");
    assert_eq!(eval_str("var k=0; for (var i in new Array(3)) {k++;} ''+k"), "0");
    assert_eq!(eval_str("new Array(2).join(',')"), ",");
    assert_eq!(eval_str("JSON.stringify(new Array(1))"), "[null]");
}

#[test]
fn new_array_hole_read_falls_through_to_prototype_getter() {
    // 洞位读落原型链：继承的索引 getter 被触发一次并返回其值。
    assert_eq!(
        eval_str("var n=0; Object.defineProperty(Array.prototype,'0',{get:function(){n++;return 42;}}); var a=new Array(1); var v=a[0]; n+':'+v"),
        "1:42"
    );
}

#[test]
fn new_array_hole_write_clears_marker() {
    // 洞位写入后恢复为 present 属性：原洞位保持缺失。
    assert_eq!(
        eval_str("var a=new Array(2); a[1]=5; ''+(0 in a)+':'+(1 in a)+':'+a.length"),
        "false:true:2"
    );
}

#[test]
fn new_array_holes_pop_shift() {
    // 全洞数组 pop/shift：返回 undefined，length 归 0。
    assert_eq!(eval_str("var a=new Array(1); var p=a.pop(); (p===undefined)+':'+a.length"), "true:0");
    assert_eq!(eval_str("var a=new Array(1); var s=a.shift(); (s===undefined)+':'+a.length"), "true:0");
}

#[test]
fn read_paths_skip_holes_per_spec() {
    // iterate 族：洞位不触发回调；map 结果在源洞位留洞、长度不变。
    assert_eq!(eval_str("var n=0; new Array(2).forEach(function(){n++;}); ''+n"), "0");
    assert_eq!(
        eval_str("var n=0; var r=[1,,3].map(function(x){n++;return x;}); n+':'+(1 in r)+':'+r.length"),
        "2:false:3"
    );
    assert_eq!(
        eval_str("var r=new Array(3).map(function(x){return x;}); (0 in r)+':'+r.length"),
        "false:3"
    );
    assert_eq!(eval_str("new Array(2).filter(function(){return true;}).length"), "0");
    // reduce/reduceRight：无初值取首个/末尾 present 位作累加器，洞位跳过；全洞抛 TypeError。
    assert_eq!(
        eval_str("[1,,].reduce(function(a,b){return b;})+':'+(function(){try{new Array(1).reduce(function(){});return 'no-throw';}catch(e){return e.name;}})()"),
        "1:TypeError"
    );
    assert_eq!(
        eval_str("[,1,2].reduceRight(function(a,b){return a+b;})+':'+(function(){try{new Array(1).reduceRight(function(){});return 'no-throw';}catch(e){return e.name;}})()"),
        "3:TypeError"
    );
    // flatMap：洞位不触发回调；嵌套数组展开丢弃洞位（紧凑结果）。
    assert_eq!(
        eval_str("new Array(1).flatMap(function(x){return [x];}).length+':'+[1].flatMap(function(){return [1,,3];}).join(',')+':'+[1].flatMap(function(){return [1,,3];}).length"),
        "0:1,3:2"
    );
    // indexOf/lastIndexOf：洞位不参与比较。
    assert_eq!(eval_str("[1,,3].indexOf(undefined)+':'+[1,,3].lastIndexOf(undefined)"), "-1:-1");
    // slice：终长 = end - start；源洞位在结果中留洞。
    assert_eq!(
        eval_str("new Array(3).slice().length+':'+[1,,3].slice(0,2).length+':'+(1 in [1,,3].slice(0,2))"),
        "3:2:false"
    );
    // concat：this 与数组实参的洞位均在结果中保洞。
    assert_eq!(
        eval_str("[1].concat([,2]).length+':'+(1 in [1].concat([,2]))+':'+[1].concat(new Array(1)).length"),
        "3:false:2"
    );
    // flat：顶层与嵌套洞位均丢弃（紧凑结果）。
    assert_eq!(eval_str("[,1].flat().length+':'+[1,[2,,3],4].flat().join(',')"), "1:1,2,3,4");
    // sort：洞位不参与比较；present 值紧凑写回，尾部转洞。
    assert_eq!(
        eval_str("var a=[,1,3,2].sort(); (0 in a)+':'+(3 in a)+':'+a.join(',')"),
        "true:false:1,2,3,"
    );
    assert_eq!(
        eval_str("var b=[5,,1,3].sort(function(x,y){return x-y;}); b.join(',')+':'+(3 in b)"),
        "1,3,5,:false"
    );
    // 防过度门控：find 族无门控（洞位照常调回调）、includes 无门控（洞 Get 得 undefined）。
    assert_eq!(
        eval_str("[,1].findIndex(function(){return true;})+':'+[1,,3].includes(undefined)+':'+[,1].findIndex(function(x){return x===1;})"),
        "0:true:1"
    );
}
