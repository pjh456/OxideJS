use oxide_parser::{
    BindingPattern, ChainElement, ClassElement, Expression, ForStatementInit, ObjectPropertyKind, PropertyKey,
    SimpleAssignmentTarget, Statement,
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

pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, false)
}

pub fn compiled_module_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, true)
}

fn hash_program(program: &oxide_parser::Program, include_binding_names: bool) -> u64 {
    use std::hash::Hasher;

    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        statement::hash_statement(stmt, &mut h, include_binding_names);
    }

    h.finish()
}

fn hash_binding_pattern(pattern: &BindingPattern, h: &mut rustc_hash::FxHasher) {
    if let BindingPattern::BindingIdentifier(ident) = pattern {
        ident.name.as_str().hash(h);
    }
}
