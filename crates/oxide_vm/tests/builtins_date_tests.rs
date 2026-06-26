use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn str_val(vm: &Vm, val: oxide_types::value::JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

// -- static methods --

#[test]
fn date_now_returns_number() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.now()").unwrap();
    assert!(r.is_double() && r.as_double() > 0.0);
}

#[test]
fn date_parse_iso_format() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('2020-01-15')").unwrap();
    assert!(!r.as_double().is_nan());
}

#[test]
fn date_parse_invalid() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('nope')").unwrap();
    assert!(r.as_double().is_nan());
}

// -- new Date() constructor via JS --

#[test]
fn date_new_multi_arg() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 15).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2020.0);
}

#[test]
fn date_new_epoch_zero() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).getTime()").unwrap();
    assert_eq!(r.as_double(), 0.0);
}

#[test]
fn date_new_get_month() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 5, 15).getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
}

#[test]
fn date_new_get_date() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 1, 29).getDate()").unwrap();
    assert_eq!(r.as_double(), 29.0);
}

#[test]
fn date_new_get_hours_min_sec() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getHours()").unwrap();
    assert_eq!(r.as_double(), 12.0);
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getMinutes()").unwrap();
    assert_eq!(r.as_double(), 30.0);
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getSeconds()").unwrap();
    assert_eq!(r.as_double(), 45.0);
}

#[test]
fn date_set_time() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setTime(1000); d.getTime()").unwrap();
    assert_eq!(r.as_double(), 1000.0);
}

#[test]
fn date_set_full_year() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setFullYear(2025); d.getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2025.0);
}

#[test]
fn date_value_of() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1).valueOf()").unwrap();
    assert!(r.as_double() > 0.0);
}

#[test]
fn date_to_iso_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1, 0, 0, 0).toISOString()").unwrap();
    let s = str_val(&vm, r);
    assert!(s.starts_with("2020-01-01"), "got: {s}");
}

#[test]
fn date_to_json() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1).toJSON()").unwrap();
    let s = str_val(&vm, r);
    assert!(s.starts_with("2020-01-01"), "got: {s}");
}

#[test]
fn date_to_string_not_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).toString()").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_to_utc_string_not_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).toUTCString()").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_objects_use_bounded_numeric_coercion() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "+new Date(0)").unwrap();
    assert_eq!(r.as_double(), 0.0);
}
