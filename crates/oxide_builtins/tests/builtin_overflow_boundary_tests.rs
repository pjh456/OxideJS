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
    let (vm, result) = eval(source)?;
    vm.lookup_str(result)
        .ok_or_else(|| "completion value is not a string".to_string())
}

fn assert_nan(source: &str) {
    let (_vm, result) = eval(source).unwrap();
    assert!(result.is_double() && result.as_double().is_nan(), "expected NaN from {source}");
}

// -Infinity 结束位：ToIntegerOrInfinity 不截断，负无穷归 0，slice 得空 buffer。
#[test]
fn array_buffer_slice_negative_infinity_end() {
    let (_vm, result) = eval("new ArrayBuffer(8).slice(2, -Infinity).byteLength").unwrap();
    assert_eq!(result.as_int(), 0);
}

// -Infinity 结束位：copyWithin 相对结束位归 0，区间为空，元素不变。
#[test]
fn typed_array_copy_within_negative_infinity_end() {
    let s = eval_str("new Uint8Array([1, 2, 3, 4, 5]).copyWithin(0, 1, -Infinity).join(',')").unwrap();
    assert_eq!(s, "1,2,3,4,5");
}

// 无穷年份入参：MakeDay 非有限分量得 NaN，日期置 Invalid。
#[test]
fn set_full_year_infinity_is_nan() {
    assert_nan("new Date(2016, 6, 7).setFullYear(Infinity)");
}

// 无穷月/日入参：Date.UTC 走 MakeDay，非有限分量得 NaN。
#[test]
fn date_utc_infinite_fields_are_nan() {
    assert_nan("Date.UTC(0, Infinity)");
    assert_nan("Date.UTC(0, 0, -Infinity)");
}

// 毫秒分量 1e300：MakeTime 组合越界，整体得 NaN（不得溢出）。
#[test]
fn set_hours_huge_millisecond_is_nan() {
    assert_nan("new Date(2016, 6, 7, 12, 30, 45, 99).setHours(0, 0, 0, 1e300)");
}

// at(-1e300)：负无穷索引越界，得 undefined 而非溢出。
#[test]
fn typed_array_at_negative_infinity() {
    let (_vm, result) = eval("new Uint8Array([1, 2, 3]).at(-1e300)").unwrap();
    assert!(result.is_undefined());
}

// lastIndexOf(2, -Infinity)：负无穷起点越界，得 -1 而非下溢。
#[test]
fn typed_array_last_index_of_negative_infinity() {
    let (_vm, result) = eval("new Int8Array([7, 2, 9]).lastIndexOf(2, -Infinity)").unwrap();
    assert_eq!(result.as_int(), -1);
}

// 日分量 0：rollover 到上一月月末（引擎内部自比，免时区依赖）。
#[test]
fn set_full_year_day_zero_rolls_to_previous_month() {
    let s = eval_str("String((function() { var d = new Date(2016, 6, 7, 11, 36, 23, 2); return d.setFullYear(2016, 6, 0) === new Date(2016, 5, 30, 11, 36, 23, 2).getTime(); })())").unwrap();
    assert_eq!(s, "true");
}

// 日分量 0 经 setMonth 同样 rollover。
#[test]
fn set_month_day_zero_rolls_to_previous_month() {
    let s = eval_str("String((function() { var d = new Date(2016, 6, 7, 11, 36, 23, 2); return d.setMonth(6, 0) === new Date(2016, 5, 30, 11, 36, 23, 2).getTime(); })())").unwrap();
    assert_eq!(s, "true");
}

// 无效日期 this 值：setFullYear 不早退，按 +0 基准得 2016-01-01。
#[test]
fn set_full_year_on_invalid_date_uses_epoch() {
    let s = eval_str("String((function() { var d = new Date(NaN); return d.setFullYear(2016) === new Date(2016, 0, 1).getTime(); })())").unwrap();
    assert_eq!(s, "true");
}

// 有限巨负起点：截断饱和成 isize::MIN 后归一化到 0，debug 构建不得取负溢出。
#[test]
fn typed_array_fill_finite_negative_huge_start() {
    let s = eval_str("new Uint8Array(4).fill(0, -1e300).join(',')").unwrap();
    assert_eq!(s, "0,0,0,0");
}
