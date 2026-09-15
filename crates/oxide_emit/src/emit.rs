//! emit：AST → IR 代码生成（parse → IR → bytecode 中段）。
//!
//! `Emitter` 提供各语法域的 emit_* 方法；核心状态集中在 `CompileCtx`
//! （见 `compile_ctx.rs`）。产出分域组合的 `IRFunction`，由
//! `oxide_ir::lower` 降为 bytecode。

/// 是否为匿名函数定义（剥括号）：函数/箭头/class 表达式。
pub fn is_anonymous_function_definition(expr: &oxide_parser::Expression) -> bool {
    match expr {
        oxide_parser::Expression::ArrowFunctionExpression(_)
        | oxide_parser::Expression::FunctionExpression(_)
        | oxide_parser::Expression::ClassExpression(_) => true,
        oxide_parser::Expression::ParenthesizedExpression(p) => is_anonymous_function_definition(&p.expression),
        _ => false,
    }
}

/// 常量池项（bytecode module 类型）re-export，供调用方构造常量。
pub use oxide_bytecode::module::Constant;
/// 变量声明种类（var/let/const），re-export 自 parser。
pub use oxide_parser::VariableDeclarationKind;
/// AST 语法树节点与运算符类型，re-export 自 parser。
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

/// 编译入口。方法按语法域组织在 `impl Emitter` 中。
pub struct Emitter {
    /// 源码是否为 `oxide_kernel::string_forge::source_escape` 产物
    /// （eval/Function 动态编译）：源文本内的孤立 surrogate 单元 / FFFD 以
    /// `\uXXXX` 转义文本承载（oxc 不可见裸单元——Rust str 无孤立 surrogate，
    /// 转义文本是唯一注入形态），反斜杠原样透传。正则字面量的源文本切片据此
    /// 经 `source_escape_to_key` 还原池键 marker 形态入池（物化时
    /// `decode_key` 还原为原始单元，`.source` 按原始源返回）；静态源切片不含
    /// 注入 marker，走 `pool_key_plain`。
    pub(crate) source_encoded: bool,
}

/// 判断 f64 是否为整数值且在 i32 范围内（整数常量编码用）。
pub fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

/// 判断表达式是否无副作用（字面量/标识符/纯二元运算等）。
/// 用于可丢弃值的优化路径。
/// 逻辑运算符快速路径的"无副作用"判定：仅字面量/标识符/this 读取安全。
/// 算术表达式（`1 / a` 等）不得判为无副作用——对象操作数强转（ToNumber 触发
/// valueOf/toString/getter）可能在运行期抛错，急切求值会破坏 `||`/`&&` 短路。
pub fn is_side_effect_free(expr: &Expression) -> bool {
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match expr {
            Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::Identifier(_)
            | Expression::RegExpLiteral(_)
            | Expression::ThisExpression(_) => {}
            Expression::ParenthesizedExpression(p) => stack.push(&p.expression),
            _ => return false,
        }
    }
    true
}

impl Emitter {
    /// 构造空 `Emitter`（静态编译口径：源文本为良形 UTF-8，无注入 marker）。
    pub fn new() -> Self {
        Self { source_encoded: false }
    }

    /// 置位编码源口径（见 [`Emitter::source_encoded`]）：仅动态编译入口
    /// （eval / Function 构造器）经编译器设置。
    pub fn with_source_encoded(mut self, enable: bool) -> Self {
        self.source_encoded = enable;
        self
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}
