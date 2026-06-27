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
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// Assertions stay inside JS and resolve to a boolean/number, because JsValue Display
// reveals numbers/bools but not string/object contents.

// ---- for-of loop over Map/Set (default iterators) ----

#[test]
fn for_of_map_yields_key_value_entries() {
    assert_eq!(
        eval("var m=new Map();m.set('a',1);m.set('b',2);var r='';for(var e of m){r=r+e[0]+e[1];}r==='a1b2'"),
        "true",
        "for-of over Map yields [key, value] entries in insertion order"
    );
}

#[test]
fn for_of_set_yields_values() {
    assert_eq!(
        eval("var s=new Set();s.add(10);s.add(20);var r=0;for(var v of s){r=r+v;}r===30"),
        "true",
        "for-of over Set yields values"
    );
}

#[test]
fn for_of_empty_map_runs_zero_times() {
    assert_eq!(eval("var m=new Map();var r=0;for(var e of m){r=r+1;}r===0"), "true");
}

#[test]
fn for_of_empty_set_runs_zero_times() {
    assert_eq!(eval("var s=new Set();var r=0;for(var v of s){r=r+1;}r===0"), "true");
}

// ---- Map.prototype.{entries,values,keys} ----

#[test]
fn map_values_iterator_yields_values() {
    assert_eq!(
        eval("var m=new Map();m.set('a',5);var it=m.values();var r=it.next();r.value===5 && r.done===false"),
        "true"
    );
}

#[test]
fn map_entries_iterator_yields_pairs() {
    assert_eq!(
        eval("var m=new Map();m.set('a',5);var e=m.entries().next().value;e[0]==='a' && e[1]===5"),
        "true"
    );
}

#[test]
fn map_keys_iterator_yields_keys() {
    assert_eq!(eval("var m=new Map();m.set('a',5);m.keys().next().value==='a'"), "true");
}

#[test]
fn map_iterator_reports_done_at_end() {
    assert_eq!(eval("var m=new Map();var it=m.values();it.next().done===true"), "true");
}

// ---- Set.prototype.{entries,values,keys} ----

#[test]
fn set_values_iterator_yields_values() {
    assert_eq!(eval("var s=new Set();s.add(7);s.values().next().value===7"), "true");
}

#[test]
fn set_entries_iterator_yields_value_value_pairs() {
    assert_eq!(
        eval("var s=new Set();s.add(7);var e=s.entries().next().value;e[0]===7 && e[1]===7"),
        "true"
    );
}

#[test]
fn set_keys_iterator_is_values_alias() {
    assert_eq!(eval("var s=new Set();s.add(7);s.keys().next().value===7"), "true");
}

// ---- regression: existing iterables still work; non-iterables still throw ----

#[test]
fn for_of_array_still_iterates() {
    assert_eq!(eval("var r=0;for(var v of [1,2,3]){r=r+v;}r===6"), "true");
}

#[test]
fn for_of_string_still_iterates() {
    assert_eq!(eval("var r=0;for(var c of 'abc'){r=r+1;}r===3"), "true");
}

#[test]
fn for_of_non_iterable_throws() {
    let out = eval("for(var v of 42){}");
    assert!(out.contains("TypeError"), "expected TypeError for non-iterable, got: {out}");
}
