use super::*;

impl Compiler {
    pub(crate) fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(bin) => {
                let checkpoint = ctx.reg_checkpoint();
                self.count_expression(&bin.left, ctx);
                self.count_expression(&bin.right, ctx);
                ctx.projected_pc += 1; // ADD/SUB/MUL/DIV/etc.
                if is_side_effect_free(&bin.left) && is_side_effect_free(&bin.right) {
                    ctx.restore_reg_checkpoint(checkpoint.saturating_add(1));
                }
            }
            Expression::PrivateInExpression(pin) => {
                self.count_expression(&pin.right, ctx);
                ctx.count_private_access();
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
                                ctx.count_delete_static();
                            }
                            Expression::ComputedMemberExpression(member) => {
                                self.count_expression(&member.object, ctx);
                                self.count_expression(&member.expression, ctx);
                                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
                            }
                            Expression::ChainExpression(chain) => {
                                match &chain.expression {
                                    ChainElement::StaticMemberExpression(member) => {
                                        self.count_expression(&member.object, ctx);
                                        if member.optional {
                                            self.count_optional_guard(ctx);
                                        }
                                        ctx.count_delete_static();
                                    }
                                    ChainElement::ComputedMemberExpression(member) => {
                                        self.count_expression(&member.object, ctx);
                                        if member.optional {
                                            self.count_optional_guard(ctx);
                                        }
                                        self.count_expression(&member.expression, ctx);
                                        ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
                                    }
                                    _ => {}
                                }
                                ctx.projected_pc += 1; // JMP over nullish true writer
                                ctx.projected_pc += 1; // LOAD_CONST true
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        self.count_expression(&un.argument, ctx);
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
                    ctx.count_call_instr_with_arg_ext(); // SUPER_CALL + arg_count ext word
                    return;
                }
                match &call.callee {
                    Expression::PrivateFieldExpression(member) => {
                        self.count_expression(&member.object, ctx);
                        ctx.count_private_access();
                    }
                    Expression::StaticMemberExpression(member) => {
                        if matches!(&member.object, Expression::Super(_)) {
                            ctx.count_load_var(); // this register
                            ctx.count_load_const(); // key
                            ctx.alloc_reg(); // callee
                            ctx.count_instr(); // SUPER_GET_PROP
                        } else {
                            self.count_expression(&member.object, ctx);
                            ctx.count_load_const(); // key
                            ctx.count_load_var(); // callee object copy
                            ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
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
                ctx.count_call_instr_with_arg_ext(); // CALL/CALL_NATIVE + arg_count ext word
                ctx.count_load_var(); // result <- regs[0]
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
                    if let Some(logical_op) = assign.operator.to_logical_operator() {
                        let id = ctx.next_label_id();
                        self.count_expression(&member.object, ctx);
                        ctx.count_load_const(); // key
                        ctx.count_load_var(); // current value copy
                        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext
                        self.count_logical_assign_test(logical_op, id, ctx);
                        self.count_expression(&assign.right, ctx);
                        ctx.count_ic_set_with_ext(); // IC_SET_PROP + 3 ext
                        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
                        ctx.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
                        return;
                    }
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&assign.right, ctx);
                    if assign.operator != AssignmentOperator::Assign {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1;
                    }
                    ctx.count_load_const(); // key
                    ctx.count_ic_set_with_ext(); // IC_SET_PROP or COMPOUND_MEMBER_* + 3 ext words
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
                    if let Some(logical_op) = assign.operator.to_logical_operator() {
                        let id = ctx.next_label_id();
                        self.count_expression(&member.object, ctx);
                        self.count_expression(&member.expression, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // GET_PROP_DYNAMIC
                        self.count_logical_assign_test(logical_op, id, ctx);
                        self.count_expression(&assign.right, ctx);
                        ctx.projected_pc += 1; // SET_PROP_DYNAMIC
                        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
                        ctx.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
                        return;
                    }
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&member.expression, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // SET_PROP_DYNAMIC
                } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.count_load_const(); // private id
                    ctx.count_instr(); // SET_PRIVATE
                } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(_) = &assign.left {
                    if let Some(logical_op) = assign.operator.to_logical_operator() {
                        let id = ctx.next_label_id();
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // LOAD_VAR result <- current var
                        self.count_logical_assign_test(logical_op, id, ctx);
                        self.count_expression(&assign.right, ctx);
                        ctx.projected_pc += 1; // STORE_VAR
                        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
                        ctx.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
                    } else {
                        self.count_expression(&assign.right, ctx);
                        if assign.operator != AssignmentOperator::Assign {
                            ctx.projected_pc += 1; // COMPOUND_* on var
                        } else {
                            ctx.alloc_reg();
                            ctx.projected_pc += 1; // STORE_VAR
                        }
                    }
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
                    ctx.projected_pc += 1; // JMP_IF_FALSE, JMP_IF_TRUE, or JMP_IF_NULLISH
                    if matches!(log.operator, LogicalOperator::Coalesce) {
                        ctx.projected_pc += 1; // JMP over RHS on non-nullish
                        ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
                    }
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
            Expression::ChainExpression(chain) => {
                let id = ctx.next_label_id();
                self.count_chain_element(&chain.expression, true, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_VAR result <- chain value
                ctx.projected_pc += 1; // JMP over short-circuit writer
                ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
                ctx.projected_pc += 1; // LOAD_CONST undefined
                ctx.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
            }
            Expression::ObjectExpression(obj) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_OBJECT
                let prop_checkpoint = ctx.reg_checkpoint();
                for prop in &obj.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        if matches!(p.kind, oxide_parser::PropertyKind::Get | oxide_parser::PropertyKind::Set) {
                            self.count_expression(&p.value, ctx);
                            ctx.count_load_const(); // undefined getter/setter placeholder
                            ctx.count_define_accessor(); // DEFINE_ACCESSOR + ext
                        } else {
                            ctx.alloc_reg();
                            ctx.projected_pc += 1; // LOAD_CONST key
                            self.count_expression(&p.value, ctx);
                            ctx.projected_pc += 1; // SET_PROP
                        }
                        ctx.restore_reg_checkpoint(prop_checkpoint);
                    }
                }
            }
            Expression::TemplateLiteral(tl) => {
                // TemplateLiteral: N quasis (strings) + M expressions interleaved.
                // Allocate 1 result reg + count expressions.
                for expr in &tl.expressions {
                    self.count_expression(expr, ctx);
                }
                let segment_count = tl.quasis.len() + tl.expressions.len();
                ctx.count_template_str(segment_count);
            }
            Expression::TaggedTemplateExpression(tt) => {
                // Tagged template: tag expr + quasis as LOAD_CONST + expression args + CALL
                // Consecutive arg registers at end (via LOAD_VAR copies)
                self.count_expression(&tt.tag, ctx);
                let quasi_count = tt.quasi.quasis.len();
                // Each quasi: LOAD_CONST string + LOAD_CONST index + SET_ELEM
                // for both cooked and raw arrays
                for _ in 0..quasi_count {
                    ctx.count_load_const(); // string
                    ctx.count_load_const(); // index
                    ctx.count_instr(); // SET_ELEM (cooked)
                }
                // Cooked array: alloc + NEW_ARRAY
                ctx.alloc_reg(); // cooked_temp
                ctx.count_instr(); // NEW_ARRAY

                for _ in 0..quasi_count {
                    ctx.count_load_const(); // string
                    ctx.count_load_const(); // index
                    ctx.count_instr(); // SET_ELEM (raw)
                }
                // Raw array: alloc + NEW_ARRAY
                ctx.alloc_reg(); // raw_temp
                ctx.count_instr(); // NEW_ARRAY

                // Count expression arguments
                for expr in &tt.quasi.expressions {
                    self.count_expression(expr, ctx);
                }

                // Consecutive arg slots: cooked_slot, raw_slot, N expr_slots
                ctx.alloc_reg(); // cooked_slot
                ctx.alloc_reg(); // raw_slot
                for _ in &tt.quasi.expressions {
                    ctx.alloc_reg(); // expr_slot
                    ctx.count_instr(); // LOAD_VAR
                }
                // LOAD_VAR for cooked and raw
                ctx.count_words(2);

                // undefined this arg
                ctx.count_load_const();

                // CALL + ext word
                ctx.count_call_instr_with_arg_ext();

                // Result reg + LOAD_VAR
                ctx.count_load_var();
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
                ctx.count_instr_with_ext(1); // NEW_EXPRESSION + arg_count ext word
            }
            Expression::ArrayExpression(arr) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_ARRAY
                let elem_checkpoint = ctx.reg_checkpoint();
                for elem in &arr.elements {
                    if let Some(e) = elem.as_expression() {
                        self.count_expression(e, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // LOAD_CONST index
                        ctx.projected_pc += 1; // SET_ELEM
                        ctx.restore_reg_checkpoint(elem_checkpoint);
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    ctx.count_load_const(); // key
                    ctx.count_load_var(); // this
                    ctx.alloc_reg(); // result
                    ctx.count_instr(); // SUPER_GET_PROP
                    return;
                }
                self.count_expression(&member.object, ctx);
                ctx.count_load_const(); // key
                ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // GET_PROP_DYNAMIC
            }
            Expression::PrivateFieldExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_private_access();
            }
            Expression::ParenthesizedExpression(p) => {
                self.count_expression(&p.expression, ctx);
            }
            Expression::ThisExpression(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_VAR from reg 254
            }
            Expression::Identifier(ident) => {
                if CompileCtx::is_known_builtin(ident.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(ident.name.as_str());
                }
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_VAR
            }
            Expression::UpdateExpression(update) => match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    self.count_expression(&member.object, ctx);
                    ctx.count_load_const(); // key
                    ctx.alloc_reg();
                    ctx.count_ic_instr_with_ext(); // MEMBER_INC/MEMBER_DEC + 3 ext words
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
