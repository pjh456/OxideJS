#![doc = "OxideJS - JavaScript parser (oxc_parser re-export)"]

mod error;

pub use error::OxideError;
pub use oxc_allocator::Allocator;
pub use oxc_ast::ast::*;
pub use oxc_span::Span;

pub fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    let source_type = SourceType::default();
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

    Ok(ret.program)
}
