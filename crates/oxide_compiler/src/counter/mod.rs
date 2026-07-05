#[allow(unused_imports)]
use crate::compiler::{
    is_int_literal, is_side_effect_free, BinaryOperator, CompileCtx, Compiler, FunctionBodyContext, Label, ParamSpec,
};
#[allow(unused_imports)]
use oxide_bytecode::module::Constant;
#[allow(unused_imports)]
use oxide_bytecode::opcode::{self, OpCode};
#[allow(unused_imports)]
use oxide_parser::{
    AssignmentOperator, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty, BindingPattern,
    ChainElement, Class, ClassElement, Expression, ForStatementInit, ForStatementLeft, LogicalOperator,
    MethodDefinitionKind, ObjectAssignmentTarget, PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement,
    UnaryOperator, UpdateOperator, VariableDeclarationKind,
};

pub mod helper;
