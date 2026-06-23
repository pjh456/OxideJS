use oxide_parser::{
    AssignmentOperator, ChainElement, ClassElement, Expression, ForStatementInit, LogicalOperator,
    MethodDefinitionKind, SimpleAssignmentTarget, Statement, UnaryOperator, VariableDeclarationKind,
};

use crate::compiler::{is_side_effect_free, CompileCtx, Compiler, Label};

mod expr;
mod helper;
mod stmt;
