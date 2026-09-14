#![doc = "OxideJS - JavaScript parser (oxc_parser re-export)"]

mod error;
mod parser_log;

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
/// 语法错误）则透传其已收集的诊断（消息 + 位置），无诊断时返回单个通用错误。
fn parse_with_source_type<'a>(
    allocator: &'a Allocator, source: &'a str, source_type: oxc_span::SourceType,
) -> Result<Program<'a>, Vec<OxideError>> {
    use oxc_parser::Parser;

    let ret = Parser::new(allocator, source, source_type).parse();

    if ret.panicked {
        // 不可恢复语法错误：oxc 已为出错 token 累积诊断时按既有路径透传
        // （消息 + 位置），直指真实触发点；无诊断时兜底通用文本。
        if !ret.errors.is_empty() {
            return Err(ret.errors.into_iter().map(OxideError::from).collect());
        }
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
    let semantic_ret = oxc_semantic::SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&ret.program);
    if !semantic_ret.errors.is_empty() {
        return Err(semantic_ret.errors.into_iter().map(OxideError::from).collect());
    }

    Ok(ret.program)
}

/// 将 `source` 解析为 AST 根节点 `Program`（脚本/模块由语法内容自动判定）。
///
/// 使用 oxc 的 `unambiguous` SourceType：代码含顶层 import/export 时按模块解析，
/// 否则按脚本解析。返回 `Ok(program)` 或 `Err`（收集全部解析错误）。
pub fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    parse_with_source_type(allocator, source, oxc_span::SourceType::unambiguous())
}

/// 将 `source` 强制按 ES module 解析为 AST 根节点 `Program`。
///
/// 与 [`parse`] 的唯一区别是 SourceType 固定为模块（`SourceType::mjs`）：
/// 即使源码没有 import/export 也会按模块文法解析（如 `await` 保留字、
/// module-only early errors 等），用于 test262 `flags: [module]` 用例。
pub fn parse_module<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    parse_with_source_type(allocator, source, oxc_span::SourceType::mjs())
}
