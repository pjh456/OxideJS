use std::hash::{Hash, Hasher};
use std::sync::Arc;

use dashmap::DashMap;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::module::CompiledModule;
use oxide_parser::{Expression, Statement};

pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        hash_statement(stmt, &mut h);
    }

    h.finish()
}

fn hash_statement(stmt: &Statement, h: &mut rustc_hash::FxHasher) {
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

fn hash_expression(expr: &Expression, h: &mut rustc_hash::FxHasher) {
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
        }
    }
}

pub struct CodeForge {
    map: DashMap<u64, Arc<CompiledModule>>,
}

impl CodeForge {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub fn get_or_compile(
        &self,
        program: &oxide_parser::Program,
        compiler: &Compiler,
    ) -> Result<Arc<CompiledModule>, String> {
        let hash = structural_hash(program);

        if let Some(module) = self.map.get(&hash) {
            return Ok(Arc::clone(&module));
        }

        let module = compiler.compile(program)?;
        let module = Arc::new(module);

        Ok(Arc::clone(self.map.entry(hash).or_insert(module).value()))
    }
}

impl Default for CodeForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_parser::Allocator;

    #[test]
    fn test_structural_hash_same_input() {
        let a1 = Allocator::default();
        let a2 = Allocator::default();
        let p1 = oxide_parser::parse(&a1, "1 + 2").expect("parse failed");
        let p2 = oxide_parser::parse(&a2, "1 + 2").expect("parse failed");
        assert_eq!(structural_hash(&p1), structural_hash(&p2));
    }

    #[test]
    fn test_structural_hash_same_structure() {
        let a1 = Allocator::default();
        let a2 = Allocator::default();
        let p1 = oxide_parser::parse(&a1, "var x = 1; var y = 2;").expect("parse failed");
        let p2 = oxide_parser::parse(&a2, "var a = 3; var b = 4;").expect("parse failed");
        assert_eq!(structural_hash(&p1), structural_hash(&p2));
    }

    #[test]
    fn test_structural_hash_different_ops() {
        let a1 = Allocator::default();
        let a2 = Allocator::default();
        let p1 = oxide_parser::parse(&a1, "1 + 2").expect("parse failed");
        let p2 = oxide_parser::parse(&a2, "1 - 2").expect("parse failed");
        assert_ne!(structural_hash(&p1), structural_hash(&p2));
    }

    #[test]
    fn test_cache_hit() {
        let forge = CodeForge::new();
        let compiler = Compiler::new();
        let allocator = Allocator::default();
        let program = oxide_parser::parse(&allocator, "1 + 2").expect("parse failed");

        let first = forge
            .get_or_compile(&program, &compiler)
            .expect("first compile");
        let second = forge
            .get_or_compile(&program, &compiler)
            .expect("second compile");

        assert_eq!(
            (first.bytecode.len(), first.n_registers),
            (second.bytecode.len(), second.n_registers)
        );
    }

    #[test]
    fn test_cache_miss_new_program() {
        let forge = CodeForge::new();
        let compiler = Compiler::new();

        let a1 = Allocator::default();
        let p1 = oxide_parser::parse(&a1, "1 + 2").expect("parse failed");
        let a2 = Allocator::default();
        let p2 = oxide_parser::parse(&a2, "3 * 4").expect("parse failed");

        let m1 = forge.get_or_compile(&p1, &compiler).expect("compile p1");
        let m2 = forge.get_or_compile(&p2, &compiler).expect("compile p2");

        assert_ne!(m1.bytecode, m2.bytecode);
    }
}
