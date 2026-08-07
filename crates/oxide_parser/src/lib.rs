#![doc = "OxideJS - JavaScript parser (oxc_parser re-export)"]

mod error;

/// 解析错误：消息 + 源文件字节区间（起始，结束）。
pub use error::OxideError;
/// 重导出 `oxc_allocator::Allocator`：AST 节点的 bump 分配器，解析产物生命周期由其决定。
pub use oxc_allocator::Allocator;
/// 重导出 `oxc_ast::ast` 的全部 AST 节点类型（`Program`、`Statement`、`Expression` 等）。
pub use oxc_ast::ast::*;
/// 重导出 `oxc_span::Span`：源码中的位置区间。
pub use oxc_span::Span;

/// 把 `source` 解析为 AST 根节点 `Program`，AST 分配在 `allocator` 中。
///
/// 使用 oxc 默认 `SourceType`（严格模式由语法自身决定）。返回 `Ok(program)`
/// 或 `Err`（收集全部解析错误，按源码顺序排列）；若 oxc 内部 panic（不可恢复
/// 语法错误）则返回单个通用错误。
pub fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    let source_type = SourceType::unambiguous();
    let ret = Parser::new(allocator, source, source_type).parse();

    if ret.panicked {
        return Err(vec![OxideError {
            message: "Parser panicked: unrecoverable syntax error".to_string(),
            span: (0, 0),
        }]);
    }

    if !ret.errors.is_empty() {
        return Err(ret.errors.into_iter().map(OxideError::from).collect());
    }

    // 语法早期错误（Early Errors）检查：oxc parser 把复杂早期错误委托给
    // oxc_semantic（strict 绑定标识符、重复参数、NSPL、super 位置等）。
    let semantic_ret =
        oxc_semantic::SemanticBuilder::new().with_check_syntax_error(true).build(&ret.program);
    if !semantic_ret.errors.is_empty() {
        return Err(semantic_ret.errors.into_iter().map(OxideError::from).collect());
    }

    Ok(ret.program)
}
