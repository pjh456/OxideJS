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

#[test]
fn parse_concise_arrow_array_member_call() {
    // concise 箭头体 + 数组字面量 + 成员调用形是合法 JS：必须解析成功。
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, "() => [1, 2].join('|')");
    assert!(result.is_ok(), "concise 箭头体数组成员调用形应解析成功，实际错误：{:?}", result.err());
}

#[test]
fn parse_concise_arrow_iife_with_kind_helper() {
    // 顶层 kind 助手 + concise IIFE 九元素数组 .join('|') 形（事故测试源形态复刻）：
    // 必须解析成功；若解析失败，错误消息应携带定位到具体出错 token 的诊断。
    let allocator = Allocator::default();
    let source = "const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
(() => [
  kind(() => new Temporal.PlainDate(2020n, 1, 1)),
  kind(() => new Temporal.PlainDate(Symbol(), 1, 1)),
  kind(() => new Temporal.PlainDate(undefined, 1, 1)),
  kind(() => new Temporal.PlainDate(2020, 'invalid', 1)),
  kind(() => new Temporal.PlainDate(Infinity, 1, 1)),
  kind(() => new Temporal.PlainDate(2020, 1, -Infinity)),
  kind(() => new Temporal.PlainDate()),
  kind(() => new Temporal.PlainDate(2021)),
  kind(() => Temporal.PlainDate(2020, 1, 1)),
].join('|'))()";
    let result = oxide_parser::parse(&allocator, source);
    assert!(result.is_ok(), "concise IIFE 九元素数组 join 形应解析成功，实际错误：{:?}", result.err());
}
