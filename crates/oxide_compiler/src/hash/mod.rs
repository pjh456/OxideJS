//! AST 结构哈希：按语法域（statement/expression/class/...）递归计算
//! `Program` 的稳定哈希，用作编译缓存键（code cache）。
//!
//! `structural_hash` 忽略绑定名（结构相等即可命中），
//! `compiled_module_hash` 纳入绑定名（精确匹配避免错误复用）。

use oxide_parser::{
    ArrayExpressionElement, BindingPattern, ChainElement, Class, ClassElement, Declaration,
    ExportDefaultDeclarationKind, Expression, ForStatementInit, ForStatementLeft, Function, ImportDeclarationSpecifier,
    ModuleExportName, ObjectPropertyKind, PropertyKey, SimpleAssignmentTarget, Statement,
};
use std::hash::Hash;

#[derive(Hash)]
enum HashDomain {
    Statement,
    Expression,
    ClassElement,
    PropertyKey,
    ObjectPropertyKind,
    SimpleAssignmentTarget,
}

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

/// 结构哈希：忽略绑定名的 `Program` 哈希（用于粗粒度缓存命中判断）。
pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, false)
}

/// 编译模块哈希：纳入绑定名的 `Program` 哈希（用于精确缓存命中判断）。
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

fn hash_binding_pattern(pattern: &BindingPattern, h: &mut rustc_hash::FxHasher) {
    if let BindingPattern::BindingIdentifier(ident) = pattern {
        ident.name.as_str().hash(h);
    }
}
