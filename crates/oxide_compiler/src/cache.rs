use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use oxide_parser::{Expression, Statement};
use rustc_hash::FxHasher;

use crate::compiler::Compiler;
use crate::module::CompiledModule;

/// Hash AST structure — strips identifiers and literal values.
/// Two scripts with different variable names but identical tree shape
/// produce the same hash, enabling structural cache reuse.
pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    let mut h = FxHasher::default();

    for stmt in &program.body {
        hash_statement(stmt, &mut h);
    }

    h.finish()
}

fn hash_statement(stmt: &Statement, h: &mut FxHasher) {
    match stmt {
        Statement::ExpressionStatement(es) => {
            0u8.hash(h);
            hash_expression(&es.expression, h);
        }
        Statement::VariableDeclaration(decl) => {
            1u8.hash(h);
            (decl.declarations.len() as u32).hash(h);
        }
        Statement::ReturnStatement(ret) => {
            2u8.hash(h);
            if let Some(arg) = &ret.argument {
                hash_expression(arg, h);
            }
        }
        Statement::IfStatement(ifs) => {
            3u8.hash(h);
            hash_expression(&ifs.test, h);
            hash_statement(&ifs.consequent, h);
            if let Some(alt) = &ifs.alternate {
                hash_statement(alt, h);
            }
        }
        Statement::WhileStatement(wh) => {
            4u8.hash(h);
            hash_expression(&wh.test, h);
            hash_statement(&wh.body, h);
        }
        Statement::ForStatement(fr) => {
            5u8.hash(h);
            hash_statement(&fr.body, h);
        }
        Statement::BlockStatement(block) => {
            6u8.hash(h);
            for s in &block.body {
                hash_statement(s, h);
            }
        }
        _ => {
            std::mem::discriminant(stmt).hash(h);
        }
    }
}

fn hash_expression(expr: &Expression, h: &mut FxHasher) {
    match expr {
        Expression::BinaryExpression(bin) => {
            0u8.hash(h);
            std::mem::discriminant(&bin.operator).hash(h);
            hash_expression(&bin.left, h);
            hash_expression(&bin.right, h);
        }
        Expression::UnaryExpression(un) => {
            1u8.hash(h);
            std::mem::discriminant(&un.operator).hash(h);
            hash_expression(&un.argument, h);
        }
        Expression::CallExpression(_) => {
            2u8.hash(h);
        }
        Expression::LogicalExpression(log) => {
            3u8.hash(h);
            std::mem::discriminant(&log.operator).hash(h);
            hash_expression(&log.left, h);
            hash_expression(&log.right, h);
        }
        _ => {
            std::mem::discriminant(expr).hash(h);
            // strip literal values — only node type matters for structural identity
        }
    }
}

/// Local compilation cache — Phase 4 in-memory HashMap.
/// Migrated to OxideKernel CodeForge (DashMap) in Phase 7.
pub struct CodeCache {
    map: HashMap<u64, Arc<CompiledModule>>,
}

impl CodeCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get_or_compile(
        &mut self,
        program: &oxide_parser::Program,
        compiler: &Compiler,
    ) -> Result<Arc<CompiledModule>, String> {
        let hash = structural_hash(program);

        if let Some(module) = self.map.get(&hash) {
            return Ok(Arc::clone(module));
        }

        let module = compiler.compile(program)?;
        let module = Arc::new(module);
        self.map.insert(hash, Arc::clone(&module));
        Ok(module)
    }
}

impl Default for CodeCache {
    fn default() -> Self {
        Self::new()
    }
}
