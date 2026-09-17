//! AST 结构哈希：按语法域（statement/expression/class/...）递归计算
//! `Program` 的稳定哈希，用作编译缓存键（code cache）。
//!
//! `structural_hash` 为粗粒度键，忽略变量声明名、标识符读、import 本地名与
//! 函数参数等大部分绑定名；`compiled_module_hash` 为精确键，额外纳入全部绑定名。
//! 函数声明名、类名与 catch 参数标识名是编译产物物化名，两种粒度下恒计入哈希；
//! catch 参数为解构形态时不贡献哈希。

use oxide_parser::{
    ArrayExpressionElement, BindingPattern, ChainElement, Class, ClassElement, Declaration,
    ExportDefaultDeclarationKind, Expression, ForStatementInit, ForStatementLeft, Function, ImportDeclarationSpecifier,
    ModuleExportName, ObjectPropertyKind, PropertyKey, SimpleAssignmentTarget, Statement,
};
use std::hash::Hash;

/// 哈希域标识：各语法域占用独立哈希空间，避免跨类型同形节点
/// （如语句与表达式字段结构相同）发生碰撞。
#[derive(Hash)]
enum HashDomain {
    Statement,
    Expression,
    ClassElement,
    PropertyKey,
    ObjectPropertyKind,
    SimpleAssignmentTarget,
}

/// 结构哈希统一入口：先哈希域标识、再哈希变体判别符，最后分派到臂内载荷，
/// 保证跨域同形节点不碰撞。
macro_rules! hash_match {
    ($domain:expr, $value:expr, $h:expr, { $($arms:tt)* }) => {{
        $domain.hash($h);
        std::mem::discriminant($value).hash($h);
        match $value {
            $($arms)*
        }
    }};
}

mod class;
mod expression;
mod property;
mod statement;
mod target;

/// 粗粒度结构哈希：忽略变量声明名、标识符读、import 本地名与函数参数等大部分
/// 绑定名，仅保留编译产物依赖的函数声明名、类名与 catch 参数标识名，
/// 用于结构相等即命中的缓存判断。
pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, false)
}

/// 精确编译模块哈希：在粗粒度键基础上额外纳入全部绑定名，
/// 避免绑定名不同但结构相同的程序错误复用缓存。
pub fn compiled_module_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, true)
}

fn hash_program(program: &oxide_parser::Program, include_binding_names: bool) -> u64 {
    use std::hash::Hasher;

    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        statement::hash_statement(stmt, &mut h, include_binding_names);
    }

    // directives 决定脚本/函数严格模式：不纳入 hash 时 `"use strict"; f()`
    // 与 `f()` 缓存键相同，复用错误字节码（严格标志随缓存命中错配）。
    for directive in &program.directives {
        directive.directive.as_str().hash(&mut h);
    }

    h.finish()
}

/// 哈希 `BindingIdentifier` 的绑定名；只由 `include_binding_names` 为真的调用点
/// 使用（变量声明、函数与箭头参数、方法参数、catch 参数），非标识符模式
/// （数组/对象解构）不贡献哈希。
fn hash_binding_pattern(pattern: &BindingPattern, h: &mut rustc_hash::FxHasher) {
    if let BindingPattern::BindingIdentifier(ident) = pattern {
        ident.name.as_str().hash(h);
    }
}

/// 函数体哈希：statements 之后追加 directives。
///
/// directives 决定函数体严格模式（`"use strict"` 等），不纳入 hash 时嵌套
/// 函数/方法体 strict 标志随缓存键错配复用错误字节码（this 绑定语义漂移）。
fn hash_function_body(
    body: &oxide_parser::FunctionBody<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    for stmt in &body.statements {
        statement::hash_statement(stmt, h, include_binding_names);
    }
    for directive in &body.directives {
        directive.directive.as_str().hash(h);
    }
}
