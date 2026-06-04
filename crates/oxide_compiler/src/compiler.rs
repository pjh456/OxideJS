use crate::module::CompiledModule;
use crate::opcode::{self, OpCode};

pub use oxide_parser::{BinaryOperator, Expression, Statement, UnaryOperator};

/// Literal constant stored in bytecode module's constant pool.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

pub struct Compiler;

struct CompileCtx {
    bytecode: Vec<opcode::Instr>,
    constants: Vec<Constant>,
    next_reg: u8,
    max_regs: u8,
}

impl CompileCtx {
    fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            constants: Vec::new(),
            next_reg: 0,
            max_regs: 0,
        }
    }

    fn emit(&mut self, instr: opcode::Instr) {
        self.bytecode.push(instr);
    }

    fn alloc_reg(&mut self) -> u8 {
        let r = self.next_reg;
        self.next_reg += 1;
        if self.next_reg > self.max_regs {
            self.max_regs = self.next_reg;
        }
        r
    }

    fn reset_regs(&mut self) {
        self.next_reg = 0;
    }

    fn add_constant(&mut self, c: Constant) -> u16 {
        if let Some(idx) = self.constants.iter().position(|x| x == &c) {
            return idx as u16;
        }
        let idx = self.constants.len();
        self.constants.push(c);
        idx as u16
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        let mut ctx = CompileCtx::new();

        for stmt in &program.body {
            self.count_statement(stmt, &mut ctx);
        }
        ctx.max_regs = ctx.max_regs.max(1);
        ctx.reset_regs();

        let mut last_result = 0u8;
        for stmt in &program.body {
            if let Some(r) = self.emit_statement(stmt, &mut ctx)? {
                last_result = r;
            }
        }

        if last_result != 0 {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, 0, last_result, 0));
        }
        ctx.emit(opcode::encode(OpCode::HALT, 0, 0, 0));

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
        })
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Pass 1: Counter ──

impl Compiler {
    fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => {
                self.count_expression(&es.expression, ctx);
            }
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let Some(init) = &d.init {
                        self.count_expression(init, ctx);
                    }
                }
            }
            Statement::ReturnStatement(ret) => {
                if let Some(arg) = &ret.argument {
                    self.count_expression(arg, ctx);
                }
            }
            Statement::IfStatement(ifs) => {
                self.count_expression(&ifs.test, ctx);
                self.count_statement(&ifs.consequent, ctx);
                if let Some(alt) = &ifs.alternate {
                    self.count_statement(alt, ctx);
                }
            }
            Statement::WhileStatement(wh) => {
                self.count_expression(&wh.test, ctx);
                self.count_statement(&wh.body, ctx);
            }
            Statement::ForStatement(fr) => {
                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.count_expression(expr, ctx);
                    }
                }
                if let Some(test) = &fr.test {
                    self.count_expression(test, ctx);
                }
                if let Some(update) = &fr.update {
                    self.count_expression(update, ctx);
                }
                self.count_statement(&fr.body, ctx);
            }
            Statement::BlockStatement(block) => {
                for s in &block.body {
                    self.count_statement(s, ctx);
                }
            }
            _ => {}
        }
    }

    fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(bin) => {
                self.count_expression(&bin.left, ctx);
                self.count_expression(&bin.right, ctx);
                ctx.alloc_reg();
            }
            Expression::UnaryExpression(un) => {
                self.count_expression(&un.argument, ctx);
                ctx.alloc_reg();
            }
            Expression::CallExpression(call) => {
                self.count_expression(&call.callee, ctx);
                ctx.alloc_reg();
            }
            Expression::AssignmentExpression(assign) => {
                self.count_expression(&assign.right, ctx);
                ctx.alloc_reg();
            }
            Expression::ConditionalExpression(cond) => {
                self.count_expression(&cond.test, ctx);
                self.count_expression(&cond.consequent, ctx);
                self.count_expression(&cond.alternate, ctx);
                ctx.alloc_reg();
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.count_expression(e, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.count_expression(&log.left, ctx);
                self.count_expression(&log.right, ctx);
                ctx.alloc_reg();
            }
            _ => {
                ctx.alloc_reg();
            }
        }
    }
}

// ── Pass 2: Emitter ──

impl Compiler {
    fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(es) => {
                Ok(Some(self.emit_expression(&es.expression, ctx)?))
            }
            Statement::VariableDeclaration(decl) => {
                let mut r = None;
                for d in &decl.declarations {
                    if let Some(init) = &d.init {
                        r = Some(self.emit_expression(init, ctx)?);
                    }
                }
                Ok(r)
            }
            Statement::ReturnStatement(ret) => {
                match &ret.argument {
                    Some(expr) => {
                        let r = self.emit_expression(expr, ctx)?;
                        ctx.emit(opcode::encode(OpCode::RETURN, r, 0, 0));
                    }
                    None => {
                        ctx.emit(opcode::encode(OpCode::RETURN, 0, 0, 0));
                    }
                }
                Ok(None)
            }
            Statement::BlockStatement(block) => {
                let mut r = None;
                for s in &block.body {
                    if let Some(rr) = self.emit_statement(s, ctx)? {
                        r = Some(rr);
                    }
                }
                Ok(r)
            }
            _ => Ok(None),
        }
    }

    fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => {
                let idx = ctx.add_constant(Constant::Number(n.value));
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::StringLiteral(s) => {
                let idx = ctx.add_constant(Constant::String(s.value.to_string()));
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::BooleanLiteral(b) => {
                let idx = ctx.add_constant(Constant::Boolean(b.value));
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::NullLiteral(_) => {
                let idx = ctx.add_constant(Constant::Null);
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::BinaryExpression(bin) => {
                let left = self.emit_expression(&bin.left, ctx)?;
                let right = self.emit_expression(&bin.right, ctx)?;
                let op = match bin.operator {
                    BinaryOperator::Addition => OpCode::ADD,
                    BinaryOperator::Subtraction => OpCode::SUB,
                    BinaryOperator::Multiplication => OpCode::MUL,
                    BinaryOperator::Division => OpCode::DIV,
                    BinaryOperator::Remainder => OpCode::MOD,
                    BinaryOperator::Equality => OpCode::EQ,
                    BinaryOperator::Inequality => OpCode::NEQ,
                    BinaryOperator::LessThan => OpCode::LT,
                    BinaryOperator::GreaterThan => OpCode::GT,
                    BinaryOperator::LessEqualThan => OpCode::LTE,
                    BinaryOperator::GreaterEqualThan => OpCode::GTE,
                    _ => return Err(format!("unsupported binary operator: {:?}", bin.operator)),
                };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(op, r, left, right));
                Ok(r)
            }
            Expression::UnaryExpression(un) => {
                let arg = self.emit_expression(&un.argument, ctx)?;
                if matches!(un.operator, UnaryOperator::UnaryNegation) {
                    let r = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::NEG, r, arg, 0));
                    Ok(r)
                } else {
                    Err(format!("unsupported unary operator: {:?}", un.operator))
                }
            }
            _ => Err(format!(
                "unsupported expression type: {:?}",
                std::mem::discriminant(expr)
            )),
        }
    }
}
