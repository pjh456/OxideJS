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
fn object_create_and_read() {
    let allocator = Allocator::default();
    let source = "({a:1})";
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let mut vm = Vm::new();
    let obj = vm.run(&Arc::new(module)).expect("vm run failed");
    assert!(obj.is_object());
    let obj_ref = unsafe { &*obj.as_js_object_ptr() };
    assert_eq!(obj_ref.prop_count(), 1, "object should have 1 property");
    assert!(obj_ref.get_prop_at(0).is_int());
}

#[test]
fn eval_object_property_read() {
    assert_eq!(eval("({a:1,b:2}).b"), "2");
}

#[test]
fn eval_object_missing_property() {
    assert_eq!(eval("({a:1}).b"), "undefined");
}

#[test]
fn eval_computed_const_string_key_folds_to_ic() {
    // 常量字符串键折叠为 IC 静态路径：读写/复合/模板键/链式访问语义不变。
    assert_eq!(eval("var o={a:1}; o[\"a\"]"), "1");
    assert_eq!(eval("var o={a:1}; o[\"a\"]=2, o.a"), "2");
    assert_eq!(eval("var o={a:1}; o[\"a\"] += 1, o.a"), "2");
    assert_eq!(eval("var o={a:1}; o[`a`]"), "1");
    assert_eq!(eval("var o={a:{b:4}}; o[\"a\"][\"b\"]"), "4");
    assert_eq!(eval("var o={a:2}; o?.[\"a\"]"), "2");
    assert_eq!(eval("var o={a:1}; o[\"a\"]++, o.a"), "2");
    assert_eq!(eval("var o={a:1}; o[\"a\"] &&= 5, o.a"), "5");
    assert_eq!(eval("var p={a:null}; p[\"a\"] ??= 5, p.a"), "5");
}

#[test]
fn eval_computed_const_key_dynamic_keys_stay_dynamic() {
    // 动态变量键 / 数字键维持 DYNAMIC 路径，语义一致。
    assert_eq!(eval("var s=\"a\", o={a:9}; o[s]"), "9");
    assert_eq!(eval("var arr=[1,2]; arr[\"0\"]"), "1");
    assert_eq!(eval("var arr=[1,2]; arr[0]=7, arr[0]"), "7");
}

#[test]
fn eval_computed_compound_assign_preserves_read_before_rhs() {
    // 复合赋值保规范求值序：先读属性再求值 RHS，RHS 副作用改写属性不影响被加的旧值。
    assert_eq!(eval("var o={a:1}; function f(){o.a=100; return 1;} o[\"a\"]+=f()"), "2");
    assert_eq!(eval("var o={a:5}; o[\"a\"]-=1, o.a"), "4");
    assert_eq!(eval("var o={a:5}; o[\"a\"]*=2, o.a"), "10");
    assert_eq!(eval("var o={a:5}; o[\"a\"]/=2, o.a"), "2.5");
    assert_eq!(eval("var o={a:5}; o[\"a\"]%=3, o.a"), "2");
    assert_eq!(eval("var o={a:5}; o[\"a\"]**=2, o.a"), "25");
    assert_eq!(eval("var o={a:5}; o[\"a\"]&=3, o.a"), "1");
    assert_eq!(eval("var o={a:5}; o[\"a\"]|=8, o.a"), "13");
    assert_eq!(eval("var o={a:5}; o[\"a\"]^=1, o.a"), "4");
    assert_eq!(eval("var o={a:5}; o[\"a\"]<<=1, o.a"), "10");
    assert_eq!(eval("var o={a:5}; o[\"a\"]>>=1, o.a"), "2");
    assert_eq!(eval("var o={a:5}; o[\"a\"]>>>=1, o.a"), "2");
    // 逻辑赋值同为读→短路→RHS→写：truthy 时写 RHS 值，falsy 短路 RHS 不求值。
    assert_eq!(eval("var o={a:5}; function g(){o.a=7; return 2;} o[\"a\"]&&=g(), o.a"), "2");
    assert_eq!(eval("var o={a:0}; o[\"a\"]&&=99, o.a"), "0");
    assert_eq!(eval("var o={a:5}; var hit=0; o[\"a\"]||=(hit=1), hit"), "0");
    assert_eq!(eval("var o={a:5}; var hit=0; o[\"a\"]??=(hit=1), hit"), "0");
    assert_eq!(eval("var p={a:null}; p[\"a\"]??=5, p.a"), "5");
    assert_eq!(eval("var q={a:0}; q[\"a\"]||=9, q.a"), "9");
    // 静态成员复合赋值行为不变（pre-existing 求值序偏差不在本次修复范围）。
    assert_eq!(eval("var o={a:5}; o.a += 1, o.a"), "6");
}

#[test]
fn eval_dynamic_key_compound_exp_uses_rhs() {
    // 非常量键走 DYNAMIC 路径：`**=` 的指数取 RHS（EXP 读 [A,B]，旧值 a 槽、rhs b 槽）。
    assert_eq!(eval("var o={a:5}; var k='a'; o[k]**=2, o.a"), "25");
    assert_eq!(eval("var o={a:5}; var k='a'; o[k]**=2"), "25");
    // 负 number 指数：5 ** -1 = 0.2。
    assert_eq!(eval("var o={a:5}; var k='a'; o[k]**=-1, o.a"), "0.2");
    // BigInt：结果保持 BigInt。
    assert_eq!(eval("var o={a:5n}; var k='a'; o[k]**=3n, o.a"), "BigInt(125)");
    // 负 BigInt 指数抛 RangeError；BigInt/Number 混合抛 TypeError。
    let err = eval("var o={a:5n}; var k='a'; o[k]**=-1n");
    assert!(err.contains("RangeError"), "expected RangeError, got: {err}");
    let err2 = eval("var o={a:5}; var k='a'; o[k]**=2n");
    assert!(err2.contains("TypeError"), "expected TypeError, got: {err2}");
    // 求值序：先读旧值再求值 RHS——RHS 副作用改写属性不影响被求幂的旧值。
    assert_eq!(eval("var o={a:5}; var k='a'; function f(){o.a=100; return 2;} o[k]**=f(), o.a"), "25");
}

#[test]
fn eval_computed_compound_assign_getter_order() {
    // getter 场景：先触发 getter 读取，再求值 RHS。
    assert_eq!(eval("var g={get a(){return 2}, set a(v){}}; g[\"a\"] += 1"), "3");
    // 求值序严格验证：RHS 观察到 getter 已先行（seen=1），否则 RHS 取 100。
    // 结果 v*10+seen：规范序 101，RHS 先行 1001。
    assert_eq!(
        eval("var seen=0, v=0; var g={get a(){seen=1; return v}, set a(x){v=x}}; g[\"a\"] += (seen===0?100:10); v*10+seen"),
        "101"
    );
}

#[test]
fn eval_computed_compound_assign_read_error_skips_rhs() {
    // 读取抛错时 RHS 不求值，其副作用不泄漏。
    assert_eq!(eval("var h={get a(){throw 1}}; try{h[\"a\"]+=1}catch(e){e}"), "1");
    assert_eq!(eval("var side=0; var h={get a(){throw 1}}; try{h[\"a\"]+=(side=99)}catch(e){} side"), "0");
}

#[test]
fn eval_computed_const_key_method_call_keeps_this() {
    // 方法调用接收者折叠：this 绑定保留。
    assert_eq!(eval("var o={m(){return this.x}, x:5}; o[\"m\"]()"), "5");
}

#[test]
fn eval_computed_const_key_proto_write() {
    // 写 __proto__ 键走拦截路径，原型语义不变。
    assert_eq!(eval("var o={}; o[\"__proto__\"]={b:3}, o.b"), "3");
}

#[test]
fn eval_member_inc_post() {
    assert_eq!(eval("var obj={x:1}; obj.x++; obj.x"), "2");
}

#[test]
fn eval_member_inc_post_expr() {
    assert_eq!(eval("var obj={x:1}; obj.x++"), "2");
}

#[test]
fn eval_member_dec() {
    assert_eq!(eval("var obj={x:5}; obj.x--; obj.x"), "4");
}

#[test]
fn eval_member_dec_expr() {
    assert_eq!(eval("var obj={x:5}; obj.x--"), "4");
}

#[test]
fn eval_member_inc_pre() {
    assert_eq!(eval("var obj={x:5}; ++obj.x; obj.x"), "6");
}

#[test]
fn eval_dyn_member_inc() {
    assert_eq!(eval("var obj={a:3}; var k='a'; obj[k]++"), "4");
}

#[test]
fn eval_dyn_member_inc_var() {
    assert_eq!(eval("var obj={a:3}; var k='a'; obj[k]++; obj.a"), "4");
}

#[test]
fn eval_compound_member_add() {
    assert_eq!(eval("var obj={x:5}; obj.x+=3; obj.x"), "8");
}

#[test]
fn eval_compound_member_sub() {
    assert_eq!(eval("var obj={x:10}; obj.x-=2; obj.x"), "8");
}

#[test]
fn eval_compound_member_mul() {
    assert_eq!(eval("var obj={x:2}; obj.x*=3; obj.x"), "6");
}

#[test]
fn eval_compound_member_div() {
    assert_eq!(eval("var obj={x:10}; obj.x/=2; obj.x"), "5");
}

#[test]
fn eval_compound_member_mod() {
    assert_eq!(eval("var obj={x:7}; obj.x%=3; obj.x"), "1");
}

#[test]
fn eval_compound_member_exp() {
    assert_eq!(eval("var obj={x:2}; obj.x**=3; obj.x"), "8");
}

#[test]
fn eval_compound_member_expr_val() {
    assert_eq!(eval("var obj={x:5}; var y=obj.x+=3; y"), "8");
}

#[test]
fn eval_member_multi_inc() {
    assert_eq!(eval("var obj={}; obj.x=0; obj.x++; obj.x++; obj.x"), "2");
}

#[test]
fn eval_numeric_property_key_roundtrip() {
    assert_eq!(eval("var o={}; o[1]=42; o['1']"), "42");
}

#[test]
fn eval_object_primitive_hint_key_unifies_with_numeric_key() {
    // 对象经 ToPropertyKey 的字符串结果必须走规范化：`{toString(){return "5"}}`
    // 作为键与 `o[5]`/`o["5"]` 同键，不得 intern 出分裂的字符串键。
    assert_eq!(eval("var o={}; o[{toString(){return \"5\"}}]=7; o[\"5\"]"), "7");
    assert_eq!(eval("var o={}; o[{toString(){return \"5\"}}]=7; o[5]"), "7");
    assert_eq!(eval("var o={}; o[\"5\"]=7; o[{toString(){return \"5\"}}]"), "7");
    // 字符串值 Display 为 {string}，键等价语义与数值哨兵一致。
    assert_eq!(eval("var o={}; o[{toString(){return \"5\"}}]='a'; o[\"5\"]"), "{string}");
}

#[test]
fn eval_bigint_key_unifies_with_numeric_key() {
    // 兜底原始值键（BigInt）同样走规范化：`o[5n]` 与 `o["5"]`/`o[5]` 同键。
    assert_eq!(eval("var o={}; o[5n]=7; o[\"5\"]"), "7");
    assert_eq!(eval("var o={}; o[5n]=7; o[5]"), "7");
}

#[test]
fn eval_numeric_dynamic_property_key_roundtrip() {
    assert_eq!(eval("var o={}; var k=1; o[k]=42; o['1']"), "42");
}

#[test]
fn eval_numeric_property_key_compound_update() {
    assert_eq!(eval("var o={}; o[1]=2; o[1]++; o['1']"), "3");
}

#[test]
fn eval_in_operator_coerces_numeric_key() {
    assert_eq!(eval("var o={}; o[1]=42; 1 in o"), "true");
}

#[test]
fn object_literal_getter_returns_value() {
    assert_eq!(eval("var o={ get x(){ return 7 } }; o.x"), "7");
}

#[test]
fn object_literal_setter_updates_receiver() {
    assert_eq!(eval("var o={ set x(v){ this.y=v } }; o.x=4; o.y"), "4");
}

#[test]
fn object_literal_getter_setter_pair() {
    assert_eq!(eval("var o={ get x(){ return this.y }, set x(v){ this.y=v } }; o.x=9; o.x"), "9");
}

fn eval_string(source: &str) -> String {
    let allocator = Allocator::default();
    let mut vm = Vm::new();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let result = vm.run(&Arc::new(module)).expect("vm run failed");
    vm.lookup_str(result).unwrap_or_default()
}

#[test]
fn object_literal_spread_copies_own_properties() {
    assert_eq!(eval_string("JSON.stringify({a:0,...{x:1,y:2}})"), r#"{"a":0,"x":1,"y":2}"#);
}

#[test]
fn object_literal_spread_later_definitions_override() {
    assert_eq!(eval("({...{a:1},a:9}).a"), "9");
    assert_eq!(eval("({a:1,...{a:9}}).a"), "9");
}

#[test]
fn object_literal_spread_nullish_is_empty() {
    assert_eq!(eval_string("JSON.stringify({...null})"), "{}");
    assert_eq!(eval_string("JSON.stringify({...undefined})"), "{}");
    assert_eq!(eval_string("JSON.stringify({...42})"), "{}");
}

#[test]
fn object_literal_spread_string_indexes_and_getters() {
    assert_eq!(eval_string("JSON.stringify({...'ab'})"), r#"{"0":"a","1":"b"}"#);
    assert_eq!(eval("({...{get x(){return 5}}}).x"), "5");
    assert_eq!(eval_string("JSON.stringify({...[],a:1})"), r#"{"a":1}"#);
}

#[test]
fn object_literal_batch_slot_writes_keep_key_order() {
    assert_eq!(eval_string("JSON.stringify({a:1,b:2,c:3})"), r#"{"a":1,"b":2,"c":3}"#);
    assert_eq!(eval("({a:1,b:2,c:3}).b"), "2");
}

#[test]
fn object_literal_batch_integer_and_string_keys_are_equivalent() {
    assert_eq!(eval_string("({0:'a',1:'b'})[0]"), "a");
    assert_eq!(eval_string("({0:'a'})['0']"), "a");
    assert_eq!(eval_string("({1:'x',2:'y'})['2']"), "y");
}

#[test]
fn object_literal_batch_proto_key_still_sets_prototype() {
    assert_eq!(eval("Object.getPrototypeOf({__proto__:null})"), "null");
    assert_eq!(eval("({a:1,__proto__:{x:5}}).x"), "5");
    assert_eq!(eval("({__proto__:{x:1}}).x"), "1");
}

#[test]
fn object_literal_batch_duplicate_key_falls_back() {
    assert_eq!(eval("({a:1,a:2}).a"), "2");
    assert_eq!(eval_string("JSON.stringify({a:1,a:2})"), r#"{"a":2}"#);
}

#[test]
fn object_literal_batch_accessor_after_prefix() {
    assert_eq!(eval("({a:1,get b(){return 2}}).b"), "2");
    assert_eq!(eval("var o={a:1,set b(v){this.c=v}}; o.b=9; o.c"), "9");
}

#[test]
fn object_literal_batch_nested_and_gc_survival() {
    // 嵌套字面量内层也走批；外层对象经 GC 后嵌套值仍存活。
    assert_eq!(eval("({a:{b:1}}).a.b"), "1");
    assert_eq!(eval_string("JSON.stringify({x:{y:{z:1}},n:0})"), r#"{"x":{"y":{"z":1}},"n":0}"#);
    assert_eq!(eval("var h=[]; for(var i=0;i<50;i++) h.push({a:{b:i}}); h[49].a.b"), "49");
}

#[test]
fn object_literal_batch_value_expr_throw_is_caught() {
    assert_eq!(eval("var g; try { ({a:(()=>{throw 1})(),b:2}); } catch(e) { g=e; } g"), "1");
}

#[test]
fn object_literal_batch_mixed_computed_spread_order() {
    assert_eq!(
        eval_string("var k='k',s={m:3}; JSON.stringify({a:1,[k]:2,...s})"),
        r#"{"a":1,"k":2,"m":3}"#
    );
    assert_eq!(eval("var k='k',s={m:3}; ({a:1,[k]:2,...s}).k"), "2");
}

// ── object 生成器方法 ──

// 对象字面量生成器方法：yield 顺序与 done 收敛。
#[test]
fn object_generator_method_yields_in_order() {
    assert_eq!(
        eval_string("var o = { *gen() { yield 1; yield 2; } }; var it = o.gen(); [it.next().value, it.next().value, it.next().done].join(',')"),
        "1,2,true"
    );
}

// 对象字面量生成器方法 throw：异常进挂起点，被 body 内 catch 拦截后继续 yield。
#[test]
fn object_generator_method_throw_reaches_inner_catch() {
    assert_eq!(
        eval_string("var o = { *t() { try { yield 1; } catch (e) { yield 'caught'; } } }; var it = o.t(); var r1 = it.next(); var r2 = it.throw(new Error('x')); [r1.value, r2.value].join(',')"),
        "1,caught"
    );
}
