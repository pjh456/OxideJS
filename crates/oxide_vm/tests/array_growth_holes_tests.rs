//! 数组越界增长置洞引擎钉：define 数据/访问器、SET_ELEM 快慢路径、push / unshift
//! 扩长统一走 `set_prop_at` 增长臂，越界增长新建成段按规范置为空洞而非在位
//! undefined，行为对齐 `in`、`length`、`Object.keys`、GOPD、原型链读取与既有洞保留。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, v) = eval(source)?;
    // 字符串结果须在同一 VM 上读：perm 串指向 VM 私有内核，VM drop 后指针悬垂。
    if v.is_string() {
        Ok(vm.lookup_str(v).unwrap_or_default())
    } else {
        Ok(format!("{v}"))
    }
}

// 钉 1：define 越界数据属性后中间槽为洞（`in` 为 false、length 含新边界）。
#[test]
fn define_out_of_bounds_middle_is_hole() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'5',{value:5,configurable:true,writable:true,enumerable:true}); \
             ''+(3 in a)+':'+(4 in a)+':'+(5 in a)+':'+a.length",
        )
        .unwrap(),
        "false:false:true:6",
    );
}

// 钉 2：同形下 Object.keys 不含洞下标。
#[test]
fn define_out_of_bounds_keys_skip_holes() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'5',{value:5,configurable:true,writable:true,enumerable:true}); \
             JSON.stringify(Object.keys(a))",
        )
        .unwrap(),
        "[\"0\",\"1\",\"5\"]",
    );
}

// 钉 3：洞位读回落原型链（原型有 3 下标时读回原型上的值）。
#[test]
fn define_out_of_bounds_read_falls_to_proto() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'5',{value:5,configurable:true,writable:true,enumerable:true}); \
             Array.prototype[3]='p'; ''+a[3]",
        )
        .unwrap(),
        "p",
    );
}

// 钉 4：洞位 GOPD 返回 undefined（无描述符）。
#[test]
fn define_out_of_bounds_gopd_hole_is_undefined() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'5',{value:5,configurable:true,writable:true,enumerable:true}); \
             ''+(Object.getOwnPropertyDescriptor(a,'3')===undefined)",
        )
        .unwrap(),
        "true",
    );
}

// 钉 5：SET_ELEM 越界写后中间槽为洞（`in` 为 false、length 含新边界）。
#[test]
fn set_elem_out_of_bounds_middle_is_hole() {
    assert_eq!(
        eval_str("const a=[0,1]; a[5]=9; ''+(3 in a)+':'+(4 in a)+':'+(5 in a)+':'+a.length").unwrap(),
        "false:false:true:6",
    );
}

// 钉 6：define 越界访问器属性后中间槽为洞、目标槽 getter 在场。
#[test]
fn define_out_of_bounds_accessor_middle_is_hole() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'5',{get(){return 5},configurable:true}); \
             ''+(3 in a)+':'+(Object.getOwnPropertyDescriptor(a,'5').get!==undefined)",
        )
        .unwrap(),
        "false:true",
    );
}

// 钉 7：洞位重写后恢复在位（`in` 为 true、读回写入值）。
#[test]
fn rewrite_hole_restores_present() {
    assert_eq!(eval_str("const a=[0,1]; a[5]=9; a[3]=1; ''+(3 in a)+':'+a[3]").unwrap(), "true:1",);
}

// 钉 8：越界增长不抹已有洞（delete 后的洞与新增段洞并存）。
#[test]
fn existing_hole_preserved_on_growth() {
    assert_eq!(
        eval_str("const a=[0,1]; delete a[0]; a[5]=9; ''+(0 in a)+':'+(2 in a)+':'+(5 in a)").unwrap(),
        "false:false:true",
    );
}

// 钉 9：边界恰 length 位写入不产生中间洞（push 形）。
#[test]
fn write_at_length_boundary_is_present() {
    assert_eq!(
        eval_str("const a=[0,1]; a[2]=9; ''+(2 in a)+':'+a.length+':'+JSON.stringify(Object.keys(a))").unwrap(),
        "true:3:[\"0\",\"1\",\"2\"]",
    );
}

// 钉 10：define 边界恰 length 位同形（无中间洞）。
#[test]
fn define_at_length_boundary_is_present() {
    assert_eq!(
        eval_str(
            "const a=[0,1]; Object.defineProperty(a,'2',{value:2,configurable:true,writable:true,enumerable:true}); \
             ''+(2 in a)+':'+a.length+':'+JSON.stringify(Object.keys(a))",
        )
        .unwrap(),
        "true:3:[\"0\",\"1\",\"2\"]",
    );
}

// 钉 11：push 扩长全在位（零新增元数据语义）。
#[test]
fn push_growth_all_present() {
    assert_eq!(
        eval_str("const a=[1,2]; a.push(3); ''+(0 in a)+':'+(1 in a)+':'+(2 in a)+':'+a.length").unwrap(),
        "true:true:true:3",
    );
}

// 钉 12：unshift 扩长全在位。
#[test]
fn unshift_growth_all_present() {
    assert_eq!(
        eval_str("const a=[1,2]; a.unshift(0); ''+(0 in a)+':'+(1 in a)+':'+(2 in a)+':'+a.length").unwrap(),
        "true:true:true:3",
    );
}

// 钉 13：frozen 数组越界 define 抛 TypeError（增长前先校验冻结）。
#[test]
fn define_out_of_bounds_on_frozen_throws() {
    assert_eq!(
        eval_str(
            "try { const a=[0,1]; Object.freeze(a); \
             Object.defineProperty(a,'5',{value:5,configurable:true,writable:true,enumerable:true}); 'no-throw'; } \
             catch(e) { e.constructor.name }",
        )
        .unwrap(),
        "TypeError",
    );
}

// 钉 14：length 不可写（frozen）时越界 SET_ELEM 为 no-op（sloppy）。
#[test]
fn set_elem_out_of_bounds_noop_when_length_non_writable() {
    assert_eq!(
        eval_str("const a=[0,1]; Object.freeze(a); a[5]=9; ''+a.length+':'+(5 in a)").unwrap(),
        "2:false",
    );
}

// 钉 15：防过度修——普通对象 undefined 值属性仍在位（`in` 为 true）。
#[test]
fn plain_object_undefined_value_is_present() {
    assert_eq!(eval_str("const o={}; o.k=undefined; ''+('k' in o)").unwrap(), "true");
}

// 钉 16：防过度修——数组命名属性 undefined 值仍在位（hasOwn 与 keys 均含）。
#[test]
fn array_named_undefined_value_is_present() {
    assert_eq!(
        eval_str("const a=[0,1]; a.x=undefined; ''+Object.hasOwn(a,'x')+':'+JSON.stringify(Object.keys(a))").unwrap(),
        "true:[\"0\",\"1\",\"x\"]",
    );
}
