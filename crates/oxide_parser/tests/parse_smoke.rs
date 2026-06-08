use oxide_parser::Allocator;

#[test]
fn parse_simple_expression() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "1 + 2");
    assert!(result.is_ok(), "1+2 should parse successfully");
}

#[test]
fn parse_empty_string() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "");
    assert!(result.is_ok(), "empty string should parse");
    let program = result.unwrap();
    assert!(program.body.is_empty(), "empty program should have empty body");
}

#[test]
fn parse_function_declaration() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "function foo() { return 42; }");
    assert!(result.is_ok(), "function declaration should parse");
    let program = result.unwrap();
    assert!(!program.body.is_empty(), "program with function should not be empty");
}

#[test]
fn parse_syntax_error() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "function(");
    assert!(result.is_err(), "syntax error should return Err");
    let errors = result.unwrap_err();
    assert!(!errors.is_empty(), "syntax error should produce at least one error");
}

#[test]
fn parse_variable_declaration() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "var x = 42;");
    assert!(result.is_ok(), "variable declaration should parse");
}

#[test]
fn parse_multiple_statements() {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "var a = 1; var b = 2; var c = a + b;");
    assert!(result.is_ok(), "multiple statements should parse");
    let program = result.unwrap();
    assert_eq!(program.body.len(), 3, "should have 3 statements");
}
