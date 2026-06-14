use oxide_parser::{
    AssignmentOperator, ClassElement, Expression, ForStatementInit, MethodDefinitionKind, SimpleAssignmentTarget,
    Statement, UnaryOperator, VariableDeclarationKind,
};

use crate::compiler::{is_side_effect_free, CompileCtx, Compiler, Label};

impl Compiler {
    fn count_binding_pattern(&self, pattern: &oxide_parser::BindingPattern, ctx: &mut CompileCtx) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                ctx.projected_pc += 1; // FOR_OF_INIT
                for elem in &ap.elements {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // FOR_OF_DONE
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // FOR_OF_NEXT
                    if let Some(inner) = elem {
                        self.count_binding_pattern(inner, ctx);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.count_rest_array(ctx);
                    self.count_binding_pattern(&rest.argument, ctx);
                }
                ctx.projected_pc += 1; // FOR_OF_CLOSE
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    ctx.alloc_reg();
                    ctx.projected_pc += 5; // LOAD_VAR + LOAD_CONST + IC_GET_PROP + 3 ext
                    ctx.alloc_reg();
                    self.count_binding_pattern(&prop.value, ctx);
                }
                if let Some(rest) = &op.rest {
                    ctx.alloc_reg();
                    ctx.projected_pc += 2; // REST_OBJECT + ext
                    self.count_binding_pattern(&rest.argument, ctx);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                ctx.alloc_reg();
                ctx.projected_pc += 2; // LOAD_CONST undefined + STRICT_EQ
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_expression(&ap.right, ctx);
                ctx.projected_pc += 1; // LOAD_VAR default
                self.count_binding_pattern(&ap.left, ctx);
            }
        }
    }

    fn count_rest_array(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // rest
        ctx.projected_pc += 1; // NEW_ARRAY
        ctx.alloc_reg(); // idx
        ctx.projected_pc += 1; // LOAD_CONST 0
        ctx.alloc_reg(); // has
        ctx.projected_pc += 2; // FOR_OF_DONE + JMP_IF_FALSE
        ctx.alloc_reg(); // val
        ctx.projected_pc += 1; // FOR_OF_NEXT
        ctx.projected_pc += 1; // SET_ELEM
        ctx.alloc_reg(); // inc tmp
        ctx.projected_pc += 2; // INC_PRE + JMP
    }

    fn count_array_assignment(&self, ap: &oxide_parser::ArrayAssignmentTarget, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // FOR_OF_INIT
        for elem in &ap.elements {
            ctx.alloc_reg();
            ctx.projected_pc += 1; // FOR_OF_DONE
            ctx.alloc_reg();
            ctx.projected_pc += 1; // FOR_OF_NEXT
            if elem.is_some() {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
        }
        if ap.rest.is_some() {
            self.count_rest_array(ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
        ctx.projected_pc += 1; // FOR_OF_CLOSE
    }

    fn count_object_assignment(&self, op: &oxide_parser::ObjectAssignmentTarget, ctx: &mut CompileCtx) {
        for _ in &op.properties {
            ctx.alloc_reg();
            ctx.projected_pc += 5; // LOAD_VAR + LOAD_CONST + IC_GET_PROP + 3 ext
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
        if op.rest.is_some() {
            ctx.alloc_reg();
            ctx.projected_pc += 2; // REST_OBJECT + ext
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
    }

    fn count_class(&self, class: &oxide_parser::Class, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // ctor_reg
        ctx.alloc_reg(); // proto_reg
        if let Some(super_class) = &class.super_class {
            self.count_expression(super_class, ctx);
        }
        ctx.projected_pc += 2; // LOAD_CONST ctor + NEW_OBJECT proto

        if class.super_class.is_some() {
            ctx.alloc_reg(); // parent prototype key
            ctx.alloc_reg(); // parent prototype value
            ctx.alloc_reg(); // __proto__ key
            ctx.projected_pc += 5; // LOAD_CONST + GET_PROP + LOAD_CONST + two SET_PROP
        }

        for element in &class.body.body {
            let ClassElement::MethodDefinition(method) = element else {
                continue;
            };
            let method = method.as_ref();
            match method.kind {
                MethodDefinitionKind::Constructor => {}
                MethodDefinitionKind::Method => {
                    ctx.alloc_reg(); // method_reg
                    ctx.alloc_reg(); // key_reg
                    ctx.projected_pc += 4; // LOAD_CONST method + SET_HOME_OBJECT + LOAD_CONST key + SET_PROP
                }
                MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                    ctx.alloc_reg(); // method_reg
                    ctx.alloc_reg(); // undefined getter/setter placeholder
                    ctx.projected_pc += 5; // LOAD_CONST method + SET_HOME_OBJECT + LOAD_CONST undefined + DEFINE_ACCESSOR + ext
                }
            }
        }

        ctx.alloc_reg(); // constructor key
        ctx.projected_pc += 1; // LOAD_CONST "constructor"
        ctx.projected_pc += 1; // SET_PROP proto.constructor = ctor

        ctx.alloc_reg(); // prototype key
        ctx.projected_pc += 1; // LOAD_CONST "prototype"
        ctx.projected_pc += 1; // SET_PROP ctor.prototype = proto
    }

    pub(crate) fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => {
                self.count_expression(&es.expression, ctx);
            }
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let Some(init) = &d.init {
                        self.count_expression(init, ctx);
                        self.count_binding_pattern(&d.id, ctx);
                    } else {
                        ctx.alloc_reg();
                        ctx.projected_pc += 2; // LOAD_CONST(undefined) + STORE_VAR
                    }
                }
            }
            Statement::ReturnStatement(ret) => {
                if let Some(arg) = &ret.argument {
                    self.count_expression(arg, ctx);
                }
                ctx.projected_pc += 1; // RETURN
            }
            Statement::IfStatement(ifs) => {
                let id = ctx.next_label_id();
                let else_label = Label::IfElse(id);
                let end_label = Label::IfEnd(id);

                self.count_expression(&ifs.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_statement(&ifs.consequent, ctx);
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 1; // LOAD_VAR result <- consequent
                if ifs.alternate.is_some() {
                    ctx.projected_pc += 1; // JMP (skip else)
                }
                ctx.label_map.insert(else_label, ctx.projected_pc);
                if let Some(alt_stmt) = &ifs.alternate {
                    self.count_statement(alt_stmt, ctx);
                    ctx.projected_pc += 1; // LOAD_VAR result <- alternate
                }
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::WhileStatement(wh) => {
                let id = ctx.next_label_id();
                let start_label = Label::WhileStart(id);
                let end_label = Label::WhileEnd(id);

                ctx.label_map.insert(start_label, ctx.projected_pc);
                self.count_expression(&wh.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_statement(&wh.body, ctx);
                ctx.projected_pc += 1; // JMP (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::DoWhileStatement(dw) => {
                let id = ctx.next_label_id();
                let start_label = Label::DoWhileStart(id);
                let end_label = Label::DoWhileEnd(id);

                ctx.label_map.insert(start_label, ctx.projected_pc);
                self.count_statement(&dw.body, ctx);
                self.count_expression(&dw.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_TRUE (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::ForStatement(fr) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForStart(id);
                let update_label = Label::ForUpdate(id);
                let end_label = Label::ForEnd(id);

                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.count_expression(expr, ctx);
                    } else if let ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            if let Some(init_expr) = &d.init {
                                self.count_expression(init_expr, ctx);
                                self.count_binding_pattern(&d.id, ctx);
                            } else {
                                ctx.alloc_reg();
                                ctx.projected_pc += 2; // LOAD_CONST(undefined) + STORE_VAR
                            }
                        }
                    }
                }
                ctx.label_map.insert(start_label, ctx.projected_pc);
                if let Some(test) = &fr.test {
                    self.count_expression(test, ctx);
                    ctx.projected_pc += 1; // JMP_IF_FALSE
                }
                self.count_statement(&fr.body, ctx);
                ctx.label_map.insert(update_label, ctx.projected_pc);
                if let Some(update) = &fr.update {
                    self.count_expression(update, ctx);
                }
                ctx.projected_pc += 1; // JMP (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::ForInStatement(fi) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForInStart(id);
                let end_label = Label::ForInEnd(id);

                self.count_expression(&fi.right, ctx);
                ctx.projected_pc += 1; // FOR_IN_INIT

                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.projected_pc += 3; // FOR_IN_DONE + JMP_IF_FALSE + JMP cleanup

                ctx.projected_pc += 1; // FOR_IN_NEXT
                match &fi.left {
                    oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                        for _d in &decl.declarations {
                            ctx.alloc_reg();
                            ctx.projected_pc += 1; // STORE_VAR (value from FOR_IN_NEXT)
                        }
                    }
                    oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                        ctx.alloc_reg(); // key register
                        ctx.projected_pc += 1; // STORE_VAR
                    }
                    _ => {}
                }

                self.count_statement(&fi.body, ctx);
                ctx.projected_pc += 1; // JMP back to start

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.projected_pc += 1; // FOR_IN_CLEANUP
            }
            Statement::ForOfStatement(fo) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForOfStart(id);
                let end_label = Label::ForOfEnd(id);

                self.count_expression(&fo.right, ctx);
                ctx.projected_pc += 1; // FOR_OF_INIT
                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.alloc_reg(); // has_reg
                ctx.projected_pc += 2; // FOR_OF_DONE + JMP_IF_FALSE
                ctx.alloc_reg(); // val_reg
                ctx.projected_pc += 1; // FOR_OF_NEXT
                match &fo.left {
                    oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            self.count_binding_pattern(&d.id, ctx);
                        }
                    }
                    oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1;
                    }
                    oxide_parser::ForStatementLeft::ArrayAssignmentTarget(ap) => {
                        self.count_array_assignment(ap, ctx);
                    }
                    oxide_parser::ForStatementLeft::ObjectAssignmentTarget(op) => {
                        self.count_object_assignment(op, ctx);
                    }
                    _ => {}
                }
                self.count_statement(&fo.body, ctx);
                ctx.projected_pc += 1; // JMP back
                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.projected_pc += 1; // FOR_OF_CLOSE
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                self.count_expression(&sw.discriminant, ctx);
                let compare_reg_checkpoint = ctx.reg_checkpoint();

                let cases = &sw.cases;
                for case in cases.iter() {
                    if let Some(test) = &case.test {
                        self.count_expression(test, ctx);
                        ctx.projected_pc += 1; // EQ
                        ctx.alloc_reg(); // eq result
                        ctx.projected_pc += 1; // JMP_IF_TRUE
                        ctx.restore_reg_checkpoint(compare_reg_checkpoint);
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    ctx.projected_pc += 1; // JMP to SwitchEnd (no match)
                }

                for (case_idx, case) in cases.iter().enumerate() {
                    let case_label = Label::SwitchCase(id, case_idx as u32);
                    ctx.label_map.insert(case_label, ctx.projected_pc);
                    for s in &case.consequent {
                        self.count_statement(s, ctx);
                    }
                }

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.pop_switch();
            }
            Statement::BreakStatement(_) => {
                ctx.projected_pc += 1; // JMP
            }
            Statement::ContinueStatement(_) => {
                ctx.projected_pc += 1; // JMP
            }
            Statement::BlockStatement(block) => {
                for s in &block.body {
                    self.count_statement(s, ctx);
                }
            }
            Statement::FunctionDeclaration(fd) => {
                let name = if let Some(id) = &fd.id {
                    id.name.to_string()
                } else {
                    return;
                };

                // Hoisting: declare function name as initialized
                let func_reg = ctx.alloc_reg();
                let _ = ctx.declare_initialized(&name, func_reg, VariableDeclarationKind::Var, false);

                // Body is compiled in the emit pass only.
                // FD emits LOAD_CONST(BytecodeFunc) + STORE_VAR
                ctx.projected_pc += 2;
            }
            Statement::ClassDeclaration(class) => {
                ctx.alloc_reg(); // class binding reg
                self.count_class(class, ctx);
                ctx.projected_pc += 1; // STORE_VAR binding <- ctor
            }
            Statement::ThrowStatement(ts) => {
                self.count_expression(&ts.argument, ctx);
                ctx.projected_pc += 1; // THROW
            }
            Statement::TryStatement(ts) => {
                let id = ctx.next_label_id();
                let catch_label = Label::CatchBody(id);
                let try_end_label = Label::TryEnd(id);
                let has_catch = ts.handler.is_some();
                let has_finally = ts.finalizer.is_some();

                ctx.alloc_reg(); // result_reg

                if has_finally {
                    ctx.projected_pc += 1; // TRY_FINALLY_BEGIN (before try body)
                }

                if has_catch {
                    ctx.projected_pc += 1; // TRY_BEGIN (before try body)
                }

                for s in &ts.block.body {
                    self.count_statement(s, ctx);
                }
                ctx.projected_pc += 1; // LOAD_VAR result_reg (if try body has result)

                if has_catch {
                    ctx.projected_pc += 1; // TRY_END
                }

                let jmp_needed = has_catch || has_finally;
                if jmp_needed {
                    ctx.projected_pc += 1; // JMP
                }

                ctx.label_map.insert(catch_label, ctx.projected_pc);
                if let Some(catch) = &ts.handler {
                    ctx.push_scope();
                    if let Some(_param) = &catch.param {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // STORE_VAR
                    }
                    for cs in &catch.body.body {
                        self.count_statement(cs, ctx);
                    }
                    ctx.projected_pc += 1; // LOAD_VAR result_reg (if catch body has result)
                    ctx.pop_scope();
                }

                if let Some(finally) = &ts.finalizer {
                    let finally_label = Label::FinallyBody(id);
                    ctx.label_map.insert(finally_label, ctx.projected_pc);
                    for fs in &finally.body {
                        self.count_statement(fs, ctx);
                    }
                    ctx.projected_pc += 1; // LOAD_VAR result_reg (if finally has result)
                    ctx.projected_pc += 1; // TRY_FINALLY_END
                }

                ctx.label_map.insert(try_end_label, ctx.projected_pc);
            }
            _ => {}
        }
    }

    pub(crate) fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(bin) => {
                self.count_expression(&bin.left, ctx);
                self.count_expression(&bin.right, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // ADD/SUB/MUL/DIV/etc.
            }
            Expression::UnaryExpression(un) => {
                match un.operator {
                    UnaryOperator::Delete => {
                        match &un.argument {
                            Expression::Identifier(_) => {
                                // SyntaxError at compile time, no bytecode cost
                            }
                            Expression::StaticMemberExpression(member) => {
                                self.count_expression(&member.object, ctx);
                                ctx.alloc_reg();
                                ctx.projected_pc += 2; // DELETE_PROP_STATIC + ext word
                            }
                            Expression::ComputedMemberExpression(member) => {
                                self.count_expression(&member.object, ctx);
                                self.count_expression(&member.expression, ctx);
                                ctx.alloc_reg();
                                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        self.count_expression(&un.argument, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // NEG/TYPEOF/VOID/NOT
                    }
                }
            }
            Expression::CallExpression(call) => {
                if matches!(&call.callee, Expression::Super(_)) {
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            self.count_expression(expr, ctx);
                        }
                    }
                    ctx.alloc_reg();
                    ctx.projected_pc += 2; // SUPER_CALL + arg_count ext word
                    return;
                }
                match &call.callee {
                    Expression::StaticMemberExpression(member) => {
                        if matches!(&member.object, Expression::Super(_)) {
                            ctx.alloc_reg(); // this register
                            ctx.projected_pc += 1; // LOAD_VAR this
                            ctx.alloc_reg(); // key
                            ctx.projected_pc += 1; // LOAD_CONST key
                            ctx.alloc_reg(); // callee
                            ctx.projected_pc += 1; // SUPER_GET_PROP
                        } else {
                            self.count_expression(&member.object, ctx);
                            ctx.alloc_reg();
                            ctx.projected_pc += 1;
                            ctx.alloc_reg();
                            ctx.projected_pc += 5; // LOAD_VAR + IC_GET_PROP + 3 ext words
                        }
                    }
                    _ => {
                        self.count_expression(&call.callee, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1;
                    }
                }
                for arg in &call.arguments {
                    if let Some(expr) = arg.as_expression() {
                        self.count_expression(expr, ctx);
                    }
                }
                // CALL and CALL_NATIVE both carry 1 ext word for arg_count.
                ctx.projected_pc += 2;
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 1; // LOAD_VAR result <- regs[0]
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&assign.right, ctx);
                    if assign.operator != AssignmentOperator::Assign {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1;
                    }
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // LOAD_CONST key
                    ctx.projected_pc += 1; // IC_SET_PROP or COMPOUND_MEMBER_*
                    ctx.projected_pc += 3; // 3 IC ext words
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&member.expression, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // SET_PROP_DYNAMIC
                } else if let oxide_parser::AssignmentTarget::ArrayAssignmentTarget(ap) = &assign.left {
                    self.count_expression(&assign.right, ctx);
                    self.count_array_assignment(ap, ctx);
                } else if let oxide_parser::AssignmentTarget::ObjectAssignmentTarget(op) = &assign.left {
                    self.count_expression(&assign.right, ctx);
                    self.count_object_assignment(op, ctx);
                } else {
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
            }
            Expression::ConditionalExpression(cond) => {
                let id = ctx.next_label_id();
                let else_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);

                self.count_expression(&cond.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_expression(&cond.consequent, ctx);
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 1; // LOAD_VAR result <- consequent
                ctx.projected_pc += 1; // JMP to end
                ctx.label_map.insert(else_label, ctx.projected_pc);
                self.count_expression(&cond.alternate, ctx);
                ctx.projected_pc += 1; // LOAD_VAR result <- alternate
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.count_expression(e, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.count_expression(&log.left, ctx);

                let is_simple = is_side_effect_free(&log.left) && is_side_effect_free(&log.right);

                if is_simple {
                    self.count_expression(&log.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // AND/OR
                } else {
                    use oxide_parser::LogicalOperator;
                    let id = ctx.next_label_id();
                    ctx.alloc_reg(); // dup register
                    ctx.projected_pc += 1; // LOAD_VAR (DUP)
                    ctx.projected_pc += 1; // JMP_IF_FALSE or JMP_IF_TRUE
                    self.count_expression(&log.right, ctx);
                    ctx.projected_pc += 1; // LOAD_VAR (overwrite)
                    let skip_label = match log.operator {
                        LogicalOperator::And => Label::TernaryEnd(id),
                        LogicalOperator::Or => Label::TernaryElse(id),
                        LogicalOperator::Coalesce => Label::TernaryEnd(id),
                    };
                    ctx.label_map.insert(skip_label, ctx.projected_pc);
                }
            }
            Expression::ObjectExpression(obj) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_OBJECT
                for prop in &obj.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        if matches!(p.kind, oxide_parser::PropertyKind::Get | oxide_parser::PropertyKind::Set) {
                            self.count_expression(&p.value, ctx);
                            ctx.alloc_reg(); // undefined getter/setter placeholder
                            ctx.projected_pc += 3; // LOAD_CONST undefined + DEFINE_ACCESSOR + ext
                        } else {
                            ctx.alloc_reg();
                            ctx.projected_pc += 1; // LOAD_CONST key
                            self.count_expression(&p.value, ctx);
                            ctx.projected_pc += 1; // SET_PROP
                        }
                    }
                }
            }
            Expression::TemplateLiteral(tl) => {
                // TemplateLiteral: N quasis (strings) + M expressions interleaved.
                // Allocate 1 result reg + count expressions.
                for expr in &tl.expressions {
                    self.count_expression(expr, ctx);
                }
                ctx.alloc_reg(); // result reg
                let segment_count = tl.quasis.len() + tl.expressions.len();
                // TEMPLATE_STR opcode + ext words (1 header + ceil(segments/1) per u32)
                ctx.projected_pc += 1 + 1 + segment_count;
            }
            Expression::TaggedTemplateExpression(tt) => {
                // Tagged template: tag expr + quasis as LOAD_CONST + expression args + CALL
                // Consecutive arg registers at end (via LOAD_VAR copies)
                self.count_expression(&tt.tag, ctx);
                let quasi_count = tt.quasi.quasis.len();
                // Each quasi: LOAD_CONST string + LOAD_CONST index + SET_ELEM
                // for both cooked and raw arrays
                for _ in 0..quasi_count {
                    ctx.alloc_reg(); // str_reg
                    ctx.projected_pc += 1; // LOAD_CONST string
                    ctx.alloc_reg(); // idx_reg
                    ctx.projected_pc += 1; // LOAD_CONST index
                    ctx.projected_pc += 1; // SET_ELEM (cooked)
                }
                // Cooked array: alloc + NEW_ARRAY
                ctx.alloc_reg(); // cooked_temp
                ctx.projected_pc += 1; // NEW_ARRAY

                for _ in 0..quasi_count {
                    ctx.alloc_reg(); // str_reg
                    ctx.projected_pc += 1; // LOAD_CONST string
                    ctx.alloc_reg(); // idx_reg
                    ctx.projected_pc += 1; // LOAD_CONST index
                    ctx.projected_pc += 1; // SET_ELEM (raw)
                }
                // Raw array: alloc + NEW_ARRAY
                ctx.alloc_reg(); // raw_temp
                ctx.projected_pc += 1; // NEW_ARRAY

                // Count expression arguments
                for expr in &tt.quasi.expressions {
                    self.count_expression(expr, ctx);
                }

                // Consecutive arg slots: cooked_slot, raw_slot, N expr_slots
                ctx.alloc_reg(); // cooked_slot
                ctx.alloc_reg(); // raw_slot
                for _ in &tt.quasi.expressions {
                    ctx.alloc_reg(); // expr_slot
                    ctx.projected_pc += 1; // LOAD_VAR
                }
                // LOAD_VAR for cooked and raw
                ctx.projected_pc += 2;

                // undefined this arg
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST

                // CALL + ext word
                ctx.projected_pc += 2;

                // Result reg + LOAD_VAR
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_VAR
            }
            Expression::ArrowFunctionExpression(_arrow) => {
                // Arrow functions: alloc 1 register for the function value.
                // Body is compiled in the emit pass only (same pattern as FE).
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST
            }
            Expression::FunctionExpression(_fe) => {
                // No hoisting - function created at expression position.
                // Body is compiled in the emit pass only.
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST
            }
            Expression::ClassExpression(class) => {
                self.count_class(class, ctx);
            }
            Expression::NewExpression(ne) => {
                // Count callee expression
                self.count_expression(&ne.callee, ctx);
                // Count arguments
                for arg in &ne.arguments {
                    if let Some(expr) = arg.as_expression() {
                        self.count_expression(expr, ctx);
                    }
                }
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 2; // NEW_EXPRESSION + ext word
            }
            Expression::ArrayExpression(arr) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_ARRAY
                for elem in &arr.elements {
                    if let Some(e) = elem.as_expression() {
                        self.count_expression(e, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // LOAD_CONST index
                        ctx.projected_pc += 1; // SET_ELEM
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    ctx.alloc_reg(); // key
                    ctx.projected_pc += 1; // LOAD_CONST key
                    ctx.alloc_reg(); // this
                    ctx.projected_pc += 1; // LOAD_VAR this
                    ctx.alloc_reg(); // result
                    ctx.projected_pc += 1; // SUPER_GET_PROP
                    return;
                }
                self.count_expression(&member.object, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST key
                ctx.projected_pc += 1; // IC_GET_PROP
                ctx.projected_pc += 3; // 3 IC ext words
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // GET_PROP_DYNAMIC
            }
            Expression::ParenthesizedExpression(p) => {
                self.count_expression(&p.expression, ctx);
            }
            Expression::ThisExpression(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_VAR from reg 254
            }
            Expression::UpdateExpression(update) => match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    self.count_expression(&member.object, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // MEMBER_INC/MEMBER_DEC opcode
                    ctx.projected_pc += 3; // 3 IC ext words
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&member.expression, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
                _ => {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
            },
            Expression::RegExpLiteral(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            _ => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST or LOAD_VAR
            }
        }
    }
}
