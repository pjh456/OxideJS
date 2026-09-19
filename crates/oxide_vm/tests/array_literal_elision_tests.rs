//! 数组字面量 elision（hole 元素）引擎钉：静态字面量 elision 由 emit 侧删除置洞，
//! spread 字面量 elision 由 SET_ELEM 扩长 + DELETE_PROP_DYNAMIC 置洞，
//! 行为对齐 `in`、`length`、`Object.keys`、`join`、for-in 键集与原型链读取。

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

// 钉 1：静态 elision 是真洞（`in` 为 false、`Object.keys` 不含洞下标）。
#[test]
fn static_elision_is_hole() {
    assert_eq!(eval_str("''+(0 in [,,])+':'+Object.keys([,,]).length").unwrap(), "false:0");
}

// 钉 2：界内静态 elision 读回为洞（length 含洞、`in` 为 false）。
#[test]
fn static_inner_elision_is_hole() {
    assert_eq!(eval_str("''+[1,,3].length+':'+(1 in [1,,3])").unwrap(), "3:false");
}

// 钉 3：首元素 elision（length 含洞、0 下标为洞）。
#[test]
fn static_leading_elision_is_hole() {
    assert_eq!(eval_str("''+[,1].length+':'+(0 in [,1])").unwrap(), "2:false");
}

// 钉 4：显式 undefined 元素是 present 属性（与 elision 区分）。
#[test]
fn explicit_undefined_is_present() {
    assert_eq!(eval_str("''+(0 in [undefined])").unwrap(), "true");
}

// 钉 5：静态 elision 落原型链（原型有 0 下标时 `in` 为 true、读回原型上的值）。
#[test]
fn static_elision_falls_to_proto_chain() {
    assert_eq!(eval_str("Array.prototype[0]='g'; ''+(0 in [,,])+':'+[,,][0]").unwrap(), "true:g",);
}

// 钉 6：join 跳洞（两个洞产生一个分隔符）。
#[test]
fn static_elision_join_skips_holes() {
    assert_eq!(eval_str("[,,].join(\",\")").unwrap(), ",");
}

// 钉 7：for-in 键集不含洞下标。
#[test]
fn static_elision_for_in_no_keys() {
    assert_eq!(eval_str("var c=0; for (var k in [,,]) c++; ''+c").unwrap(), "0");
}

// 钉 8：连续尾部 elision 计入 length。
#[test]
fn static_trailing_elisions_count_in_length() {
    assert_eq!(eval_str("''+[4,5,,,,].length").unwrap(), "5");
}

// 钉 9：spread 后尾逗号不产生 elision 元素（length 不变）。
#[test]
fn spread_trailing_comma_not_elision() {
    assert_eq!(eval_str("''+[...[1],].length").unwrap(), "1");
}

// 钉 10：spread 后尾逗号不产生 elision 元素（length = spread 长度）。
#[test]
fn spread_trailing_comma_not_elision_dense() {
    assert_eq!(eval_str("''+[...[1,2,3],].length").unwrap(), "3");
}

// 钉 11：spread 中间 elision 置洞并计入 length（界内形）。
#[test]
fn spread_inner_elision_is_hole() {
    assert_eq!(eval_str("const a=[0,...[1],,2]; ''+a.length+':'+(2 in a)").unwrap(), "4:false",);
}

// 钉 12：spread 空展开后界外 elision 扩长并置洞。
#[test]
fn spread_out_of_bounds_elision_extends_and_holes() {
    assert_eq!(
        eval_str("const b=[...[],,,]; ''+b.length+':'+(0 in b)+':'+(1 in b)").unwrap(),
        "2:false:false",
    );
}
