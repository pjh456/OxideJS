use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

mod class_decl;
mod function_decl;
mod var;

impl Compiler {
    pub(crate) fn emit_declaration_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::VariableDeclaration(_) => self.emit_variable_declaration_statement(stmt, ctx),
            Statement::FunctionDeclaration(_) => self.emit_function_declaration_statement(stmt, ctx),
            Statement::ClassDeclaration(_) => self.emit_class_declaration_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
