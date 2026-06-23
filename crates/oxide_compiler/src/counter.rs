use oxide_parser::{
    AssignmentOperator, ChainElement, ClassElement, Expression, ForStatementInit, LogicalOperator,
    MethodDefinitionKind, SimpleAssignmentTarget, Statement, UnaryOperator, VariableDeclarationKind,
};

use crate::compiler::{is_side_effect_free, CompileCtx, Compiler, Label};

impl Compiler {
    fn count_object_property_read_static(&self, ctx: &mut CompileCtx) {
        ctx.count_load_var(); // prop/object temp
        ctx.count_load_const(); // key reg
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
    }

    fn count_object_property_read_key(&self, key: &oxide_parser::PropertyKey, computed: bool, ctx: &mut CompileCtx) {
        if !computed {
            self.count_object_property_read_static(ctx);
            let _ = key;
            return;
        }

        ctx.count_load_var(); // prop/object temp
        match key {
            oxide_parser::PropertyKey::Identifier(ident) => {
                let name = ident.name.as_str();
                let _ = ctx.lookup_or_builtin(name);
                ctx.count_load_var(); // key
            }
            oxide_parser::PropertyKey::StringLiteral(_) => {
                ctx.count_load_const(); // key
            }
            oxide_parser::PropertyKey::NumericLiteral(_) => {
                ctx.count_load_const(); // key
            }
            oxide_parser::PropertyKey::StaticIdentifier(_) => {
                ctx.count_load_const(); // key
            }
            _ => {}
        }
        ctx.alloc_reg(); // result reg
        ctx.count_instr(); // GET_PROP_DYNAMIC
    }

    fn count_optional_guard(&self, ctx: &mut CompileCtx) {
        ctx.count_load_var(); // dup
        ctx.count_jump(); // JMP_IF_NULLISH
    }

    fn count_static_member_get_preserve_base(
        &self, member: &oxide_parser::StaticMemberExpression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        self.count_chainable_expression(&member.object, short_chain, ctx);
        if short_chain && member.optional {
            self.count_optional_guard(ctx);
        }
        ctx.count_load_const(); // key
        ctx.count_load_var(); // receiver copy
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
    }

    fn count_computed_member_get_preserve_base(
        &self, member: &oxide_parser::ComputedMemberExpression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        self.count_chainable_expression(&member.object, short_chain, ctx);
        if short_chain && member.optional {
            self.count_optional_guard(ctx);
        }
        self.count_expression(&member.expression, ctx);
        ctx.alloc_reg();
        ctx.count_instr(); // GET_PROP_DYNAMIC
    }

    fn count_chain_call(&self, call: &oxide_parser::CallExpression, short_chain: bool, ctx: &mut CompileCtx) {
        match &call.callee {
            Expression::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx);
            }
            Expression::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            _ => {
                self.count_chainable_expression(&call.callee, short_chain, ctx);
                if short_chain && call.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST undefined this
            }
        }
        if short_chain
            && call.optional
            && matches!(
                &call.callee,
                Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::PrivateFieldExpression(_)
            )
        {
            self.count_optional_guard(ctx);
        }
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.count_call_instr_with_arg_ext(); // CALL + arg_count ext word
        ctx.count_load_var(); // result <- regs[0]
    }

    fn count_chainable_expression(&self, expr: &Expression, short_chain: bool, ctx: &mut CompileCtx) {
        match expr {
            Expression::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            Expression::CallExpression(call) => self.count_chain_call(call, short_chain, ctx),
            Expression::ChainExpression(chain) => self.count_chain_element(&chain.expression, short_chain, ctx),
            _ => self.count_expression(expr, ctx),
        }
    }

    fn count_chain_element(&self, element: &ChainElement, short_chain: bool, ctx: &mut CompileCtx) {
        match element {
            ChainElement::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx)
            }
            ChainElement::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            ChainElement::CallExpression(call) => self.count_chain_call(call, short_chain, ctx),
            ChainElement::TSNonNullExpression(_) => {}
        }
    }

    fn count_logical_assign_test(&self, op: LogicalOperator, id: u32, ctx: &mut CompileCtx) {
        match op {
            LogicalOperator::And | LogicalOperator::Or => {
                ctx.projected_pc += 1;
            }
            LogicalOperator::Coalesce => {
                ctx.projected_pc += 1; // JMP_IF_NULLISH to store body
                ctx.projected_pc += 1; // JMP to end on non-nullish
                ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
            }
        }
    }

    fn count_binding_pattern(&self, pattern: &oxide_parser::BindingPattern, ctx: &mut CompileCtx) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                ctx.count_instr(); // FOR_OF_INIT
                for elem in &ap.elements {
                    ctx.alloc_reg();
                    ctx.count_instr(); // FOR_OF_DONE
                    ctx.alloc_reg();
                    ctx.count_instr(); // FOR_OF_NEXT
                    if let Some(inner) = elem {
                        self.count_binding_pattern(inner, ctx);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.count_rest_array(ctx);
                    self.count_binding_pattern(&rest.argument, ctx);
                }
                ctx.count_instr(); // FOR_OF_CLOSE
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.count_object_property_read_key(&prop.key, prop.computed, ctx);
                    self.count_binding_pattern(&prop.value, ctx);
                }
                if let Some(rest) = &op.rest {
                    ctx.alloc_reg();
                    ctx.count_instr_with_ext(1); // REST_OBJECT + excluded-keys ext
                    self.count_binding_pattern(&rest.argument, ctx);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                ctx.count_load_const(); // undefined
                ctx.count_instr(); // STRICT_EQ
                ctx.count_jump(); // JMP_IF_FALSE
                self.count_expression(&ap.right, ctx);
                ctx.count_instr(); // LOAD_VAR default
                self.count_binding_pattern(&ap.left, ctx);
            }
        }
    }

    fn count_rest_array(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // rest
        ctx.count_instr(); // NEW_ARRAY
        ctx.count_load_const(); // idx = 0
        ctx.alloc_reg(); // has
        ctx.count_instr(); // FOR_OF_DONE
        ctx.count_jump(); // JMP_IF_FALSE
        ctx.alloc_reg(); // val
        ctx.count_instr(); // FOR_OF_NEXT
        ctx.count_instr(); // SET_ELEM
        ctx.alloc_reg(); // inc tmp
        ctx.count_instr(); // INC_PRE
        ctx.count_jump(); // JMP
    }

    fn count_array_assignment(&self, ap: &oxide_parser::ArrayAssignmentTarget, ctx: &mut CompileCtx) {
        ctx.count_instr(); // FOR_OF_INIT
        for elem in &ap.elements {
            ctx.alloc_reg();
            ctx.count_instr(); // FOR_OF_DONE
            ctx.alloc_reg();
            ctx.count_instr(); // FOR_OF_NEXT
            if elem.is_some() {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
        }
        if ap.rest.is_some() {
            self.count_rest_array(ctx);
            ctx.alloc_reg();
            ctx.count_instr(); // STORE_VAR
        }
        ctx.count_instr(); // FOR_OF_CLOSE
    }

    fn count_object_assignment(&self, op: &oxide_parser::ObjectAssignmentTarget, ctx: &mut CompileCtx) {
        for prop in &op.properties {
            match prop {
                oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    self.count_object_property_read_static(ctx);
                    if let Some(default_expr) = &id.init {
                        ctx.count_load_const(); // undefined
                        ctx.count_instr(); // STRICT_EQ
                        ctx.count_jump(); // JMP_IF_FALSE
                        self.count_expression(default_expr, ctx);
                        ctx.count_instr(); // LOAD_VAR default
                    }
                    ctx.alloc_reg();
                    ctx.count_instr(); // STORE_VAR
                }
                oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                    self.count_object_property_read_key(&prop.name, prop.computed, ctx);
                    self.count_assignment_maybe_default(&prop.binding, ctx);
                }
            }
        }
        if op.rest.is_some() {
            ctx.alloc_reg();
            ctx.count_instr_with_ext(1); // REST_OBJECT + excluded-keys ext
            ctx.alloc_reg();
            ctx.count_instr(); // STORE_VAR
        }
    }

    fn count_assignment_maybe_default(
        &self, target: &oxide_parser::AssignmentTargetMaybeDefault, ctx: &mut CompileCtx,
    ) {
        match target {
            oxide_parser::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(default) => {
                ctx.count_load_const(); // undefined
                ctx.count_instr(); // STRICT_EQ
                ctx.count_jump(); // JMP_IF_FALSE
                self.count_expression(&default.init, ctx);
                ctx.count_instr(); // LOAD_VAR default
                self.count_assign_target(&default.binding, ctx);
            }
            oxide_parser::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(ap) => {
                self.count_array_assignment(ap, ctx)
            }
            oxide_parser::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(op) => {
                self.count_object_assignment(op, ctx)
            }
            oxide_parser::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
            _ => {}
        }
    }

    fn count_assign_target(&self, target: &oxide_parser::AssignmentTarget, ctx: &mut CompileCtx) {
        match target {
            oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
            oxide_parser::AssignmentTarget::ArrayAssignmentTarget(ap) => self.count_array_assignment(ap, ctx),
            oxide_parser::AssignmentTarget::ObjectAssignmentTarget(op) => self.count_object_assignment(op, ctx),
            _ => {}
        }
    }

    fn count_class(&self, class: &oxide_parser::Class, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // ctor_reg
        ctx.alloc_reg(); // proto_reg
        if let Some(super_class) = &class.super_class {
            self.count_expression(super_class, ctx);
        }
        ctx.count_instr(); // LOAD_CONST ctor
        ctx.count_instr(); // NEW_OBJECT proto

        if class.super_class.is_some() {
            ctx.alloc_reg(); // parent prototype key
            ctx.count_instr(); // LOAD_CONST
            ctx.alloc_reg(); // parent prototype value
            ctx.count_instr(); // GET_PROP
            ctx.alloc_reg(); // __proto__ key
            ctx.count_instr(); // LOAD_CONST
            ctx.count_instr(); // SET_PROP proto.__proto__
            ctx.count_instr(); // SET_PROP ctor.__proto__
        }

        ctx.count_load_const(); // constructor key
        ctx.count_instr(); // SET_PROP proto.constructor = ctor

        ctx.count_load_const(); // prototype key
        ctx.count_instr(); // SET_PROP ctor.prototype = proto

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if matches!(method.kind, MethodDefinitionKind::Constructor) {
                        continue;
                    }
                    if matches!(method.key, oxide_parser::PropertyKey::PrivateIdentifier(_)) {
                        if method.r#static {
                            ctx.count_load_const(); // private id
                            ctx.count_load_const(); // method
                            ctx.count_instr(); // INIT_PRIVATE
                        }
                        continue;
                    }
                    ctx.count_load_const(); // key_reg
                    ctx.count_load_const(); // method_reg
                    ctx.count_instr(); // SET_HOME_OBJECT
                    match method.kind {
                        MethodDefinitionKind::Method => {
                            ctx.count_instr(); // SET_PROP
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            ctx.count_load_const(); // undefined placeholder
                            ctx.count_define_accessor(); // DEFINE_ACCESSOR + ext
                        }
                        MethodDefinitionKind::Constructor => {}
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        ctx.count_load_const(); // key_reg
                        if let Some(value) = &prop.value {
                            self.count_expression(value, ctx);
                        } else {
                            ctx.count_load_const();
                        }
                        ctx.count_instr(); // SET_PROP
                    }
                }
                ClassElement::StaticBlock(block) => {
                    for stmt in &block.body {
                        self.count_statement(stmt, ctx);
                    }
                }
                ClassElement::AccessorProperty(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }
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
                        ctx.count_words(2); // LOAD_CONST(undefined) + STORE_VAR
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
                                ctx.count_words(2); // LOAD_CONST(undefined) + STORE_VAR
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
                ctx.count_instr(); // FOR_IN_INIT

                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_IN_DONE
                ctx.count_jump(); // JMP_IF_FALSE
                ctx.count_jump(); // JMP cleanup

                ctx.count_instr(); // FOR_IN_NEXT
                match &fi.left {
                    oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                        for _d in &decl.declarations {
                            ctx.alloc_reg();
                            ctx.count_instr(); // STORE_VAR
                        }
                    }
                    oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                        ctx.alloc_reg(); // key register
                        ctx.count_instr(); // STORE_VAR
                    }
                    _ => {}
                }

                self.count_statement(&fi.body, ctx);
                ctx.count_jump(); // JMP back to start

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_IN_CLEANUP
            }
            Statement::ForOfStatement(fo) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForOfStart(id);
                let end_label = Label::ForOfEnd(id);

                self.count_expression(&fo.right, ctx);
                ctx.count_instr(); // FOR_OF_INIT
                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.alloc_reg(); // has_reg
                ctx.count_instr(); // FOR_OF_DONE
                ctx.count_jump(); // JMP_IF_FALSE
                ctx.alloc_reg(); // val_reg
                ctx.count_instr(); // FOR_OF_NEXT
                match &fo.left {
                    oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            self.count_binding_pattern(&d.id, ctx);
                        }
                    }
                    oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                        ctx.alloc_reg();
                        ctx.count_instr(); // STORE_VAR
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
                ctx.count_jump(); // JMP back
                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_OF_CLOSE
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
                ctx.count_words(2);
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
