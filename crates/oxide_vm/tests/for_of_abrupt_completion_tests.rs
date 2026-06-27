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

// Spec (ECMA-262 §13.7.5): next()-throw propagates the ORIGINAL value WITHOUT calling
// return(); IteratorClose (return()) runs only on BODY abrupt completion (throw/break/return).
// return() is observed via globalThis (a function CAN read/write globalThis properties;
// it cannot write a plain outer `var` in this engine).

#[test]
fn for_of_next_throw_is_catchable_with_original_type() {
    assert_eq!(
        eval(
            "try{for(var v of {next:function(){throw new TypeError('boom');}}){}}\
             catch(e){e.name==='TypeError' && e.message==='boom'}"
        ),
        "true",
        "next() throw catchable with original error type+message"
    );
}

#[test]
fn for_of_next_throw_preserves_bare_thrown_value() {
    assert_eq!(
        eval("try{for(var v of {next:function(){throw 7;}}){}}catch(e){e===7}"),
        "true",
        "a bare thrown value is preserved, not re-wrapped"
    );
}

#[test]
fn for_of_next_throw_does_not_call_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){throw new Error('x');},return:function(){globalThis.r=true;return {};}};\
             try{for(var v of it){}}catch(e){}globalThis.r===false"
        ),
        "true",
        "return() must NOT be called when next() throws"
    );
}

#[test]
fn for_of_next_throw_without_return_still_propagates() {
    assert_eq!(
        eval("try{for(var v of {next:function(){throw new TypeError('e');}}){}}catch(e){e instanceof TypeError}"),
        "true"
    );
}

#[test]
fn for_of_body_throw_calls_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){return {value:1,done:false};},return:function(){globalThis.r=true;return {};}};\
             try{for(var v of it){throw new Error('boom');}}catch(e){}globalThis.r===true"
        ),
        "true",
        "body throw calls return() (IteratorClose)"
    );
}

#[test]
fn for_of_body_throw_propagates_body_error() {
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:function(){return {};}};\
             try{for(var v of it){throw new Error('boom');}}catch(e){e.message==='boom'}"
        ),
        "true"
    );
}

#[test]
fn for_of_body_throw_return_also_throws_body_error_wins() {
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:function(){throw new Error('B');}};\
             try{for(var v of it){throw new Error('A');}}catch(e){e.message==='A'}"
        ),
        "true",
        "when return() also throws, the body's original error wins"
    );
}

#[test]
fn for_of_break_calls_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){return {value:1,done:false};},return:function(){globalThis.r=true;return {};}};\
             for(var v of it){break;}globalThis.r===true"
        ),
        "true",
        "break calls return() (via FOR_OF_CLOSE)"
    );
}

#[test]
fn for_of_array_regression_still_iterates() {
    assert_eq!(eval("var r=0;for(var v of [1,2,3]){r=r+v;}r===6"), "true");
}

#[test]
fn for_of_string_regression_still_iterates() {
    assert_eq!(eval("var r=0;for(var c of 'abc'){r=r+1;}r===3"), "true");
}
