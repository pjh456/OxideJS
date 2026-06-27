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

// Order is asserted via a position map built inside JS, so the comparison
// returns a boolean (JsValue Display only reveals number/bool, not string
// contents). This is also tolerant of any inherited enumerable keys the
// engine appends after the object's own keys.

#[test]
fn for_in_ordering_integer_indices_first() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {b:2,a:1,'2':3,'1':4}){pos[x]=i;i=i+1;}\
             pos['1']<pos['2'] && pos['2']<pos['b'] && pos['b']<pos['a']"
        ),
        "true",
        "integer indices ascending, then string keys in insertion order"
    );
}

#[test]
fn for_in_ordering_string_only_insertion_order() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {c:1,b:2,a:3}){pos[x]=i;i=i+1;}\
             pos['c']<pos['b'] && pos['b']<pos['a']"
        ),
        "true",
        "string-only keys keep insertion order"
    );
}

#[test]
fn for_in_ordering_mixed_boundary() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {a:1,'0':2,b:3}){pos[x]=i;i=i+1;}\
             pos['0']<pos['a'] && pos['a']<pos['b']"
        ),
        "true",
        "integer index '0' enumerates before string keys"
    );
}

#[test]
fn for_in_ordering_integer_indices_are_numeric_not_lexicographic() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {'10':1,'2':2,'1':3}){pos[x]=i;i=i+1;}\
             pos['1']<pos['2'] && pos['2']<pos['10']"
        ),
        "true",
        "index order is 1,2,10 (numeric) not 1,10,2 (lexicographic)"
    );
}

#[test]
fn for_in_still_enumerates_own_keys() {
    assert_eq!(
        eval("var c=0;for(var k in {a:1,b:2}){c=c+1;}c>=2"),
        "true",
        "for-in still enumerates the object's own enumerable keys"
    );
}
