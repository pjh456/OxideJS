use crate::compiler::Label;
use crate::opcode::{self, OpCode};
use oxide_parser::{
    AssignmentOperator, Class, ClassElement, Expression, ForStatementInit, ForStatementLeft, MethodDefinitionKind,
    PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement, UnaryOperator, UpdateOperator,
    VariableDeclarationKind,
};

use crate::compiler::{is_int_literal, is_side_effect_free, BinaryOperator, CompileCtx, Compiler, FunctionBodyContext};
use crate::module::Constant;

impl Compiler {
    fn class_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::PrivateIdentifier(_) => Err("private class elements not yet supported".into()),
            _ => Err("unsupported class property key type".into()),
        }
    }

    fn emit_class(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u8, String> {
        let mut constructor_method = None;
        let mut instance_methods = Vec::new();
        let mut static_methods = Vec::new();
        let mut instance_accessors = Vec::new();
        let mut static_accessors = Vec::new();
        let is_derived = class.super_class.is_some();

        for element in &class.body.body {
            let ClassElement::MethodDefinition(method) = element else {
                return Err("class fields/accessors/static blocks not yet supported".into());
            };
            let method = method.as_ref();
            if method.computed {
                return Err("computed class methods not yet supported".into());
            }

            match method.kind {
                MethodDefinitionKind::Constructor => {
                    if constructor_method.is_some() {
                        return Err("duplicate class constructor".into());
                    }
                    constructor_method = Some(method);
                }
                MethodDefinitionKind::Method => {
                    let name = self.class_property_name(&method.key)?;
                    if method.r#static {
                        static_methods.push((method, name));
                    } else {
                        instance_methods.push((method, name));
                    }
                }
                MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                    let name = self.class_property_name(&method.key)?;
                    if method.r#static {
                        static_accessors.push((method, name));
                    } else {
                        instance_accessors.push((method, name));
                    }
                }
            }
        }

        let ctor_name = class.id.as_ref().map(|id| id.name.to_string());
        let ctor_reg = ctx.alloc_reg();
        let proto_reg = ctx.alloc_reg();
        let self_binding = ctor_name.as_deref().map(|name| vec![(name, ctor_reg)]).unwrap_or_default();
        let super_reg = if let Some(super_expr) = &class.super_class {
            Some(self.emit_expression(super_expr, ctx)?)
        } else {
            None
        };

        let saved_derived = ctx.in_derived_constructor;
        ctx.in_derived_constructor = is_derived;

        let mut ctor_module = if let Some(method) = constructor_method {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            self.compile_function_body_with_bindings(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
            )?
        } else {
            let mut module = self.compile_function_body_with_bindings(
                &[],
                &[],
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
            )?;
            if is_derived {
                module.bytecode.clear();
                module.constants.clear();
                module.n_registers = 1;
                module.bytecode.push(opcode::encode(OpCode::SUPER_CALL, 0, 0, 0));
                module.bytecode.push(0);
                module.bytecode.push(opcode::encode(OpCode::RETURN, 0, 0, 0));
            }
            module
        };
        ctx.in_derived_constructor = saved_derived;
        ctor_module.is_class_constructor = true;
        ctor_module.is_derived_constructor = is_derived;
        ctor_module.function_name = ctor_name.clone();
        ctx.sub_modules.push(ctor_module);

        let ctor_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
        ctx.emit_load_const(ctor_reg, ctor_idx);
        ctx.emit(opcode::encode(OpCode::NEW_OBJECT, proto_reg, 0, 0));

        if let Some(super_reg) = super_reg {
            let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
            let parent_proto_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(parent_proto_key_reg, proto_key_idx);
            let parent_proto_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::GET_PROP, super_reg, parent_proto_reg, parent_proto_key_reg));

            let proto_link_idx = ctx.add_constant(Constant::String("__proto__".to_string()));
            let proto_link_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(proto_link_key_reg, proto_link_idx);
            ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, parent_proto_reg, proto_link_key_reg));
            ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, super_reg, proto_link_key_reg));
        }

        for (method, method_name) in instance_methods {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            let saved_method = ctx.in_instance_method;
            ctx.in_instance_method = true;
            let mut method_module = self.compile_function_body_with_bindings(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
            )?;
            ctx.in_instance_method = saved_method;
            method_module.function_name = Some(method_name.clone());
            method_module.needs_home_object = true;
            ctx.sub_modules.push(method_module);

            let method_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
            let method_reg = ctx.alloc_reg();
            ctx.emit_load_const(method_reg, method_idx);
            ctx.emit(opcode::encode(OpCode::SET_HOME_OBJECT, method_reg, proto_reg, 0));

            let key_idx = ctx.add_constant(Constant::String(method_name));
            let key_reg = ctx.alloc_reg();
            ctx.emit_load_const(key_reg, key_idx);
            ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, method_reg, key_reg));
        }

        for (method, method_name) in instance_accessors {
            let accessor_reg = self.emit_class_method_function(method, &method_name, proto_reg, ctx, &self_binding)?;
            let undef_reg = self.emit_undefined(ctx);
            let (get_reg, set_reg) = match method.kind {
                MethodDefinitionKind::Get => (accessor_reg, undef_reg),
                MethodDefinitionKind::Set => (undef_reg, accessor_reg),
                _ => unreachable!(),
            };
            let key_idx = ctx.add_constant(Constant::String(method_name));
            ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, proto_reg, get_reg, set_reg));
            ctx.emit(key_idx as u32);
        }

        for (method, method_name) in static_methods {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            let saved_static = ctx.in_static_method;
            ctx.in_static_method = true;
            let mut method_module = self.compile_function_body_with_bindings(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
            )?;
            ctx.in_static_method = saved_static;
            method_module.function_name = Some(method_name.clone());
            method_module.needs_home_object = true;
            ctx.sub_modules.push(method_module);

            let method_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
            let method_reg = ctx.alloc_reg();
            ctx.emit_load_const(method_reg, method_idx);
            ctx.emit(opcode::encode(OpCode::SET_HOME_OBJECT, method_reg, ctor_reg, 0));

            let key_idx = ctx.add_constant(Constant::String(method_name));
            let key_reg = ctx.alloc_reg();
            ctx.emit_load_const(key_reg, key_idx);
            ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, method_reg, key_reg));
        }

        for (method, method_name) in static_accessors {
            let accessor_reg = self.emit_class_method_function(method, &method_name, ctor_reg, ctx, &self_binding)?;
            let undef_reg = self.emit_undefined(ctx);
            let (get_reg, set_reg) = match method.kind {
                MethodDefinitionKind::Get => (accessor_reg, undef_reg),
                MethodDefinitionKind::Set => (undef_reg, accessor_reg),
                _ => unreachable!(),
            };
            let key_idx = ctx.add_constant(Constant::String(method_name));
            ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, ctor_reg, get_reg, set_reg));
            ctx.emit(key_idx as u32);
        }

        let ctor_key_idx = ctx.add_constant(Constant::String("constructor".to_string()));
        let ctor_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(ctor_key_reg, ctor_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, ctor_reg, ctor_key_reg));

        let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
        let proto_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(proto_key_reg, proto_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, proto_reg, proto_key_reg));

        Ok(ctor_reg)
    }

    fn emit_undefined(&self, ctx: &mut CompileCtx) -> u8 {
        let idx = ctx.add_constant(Constant::Undefined);
        let reg = ctx.alloc_reg();
        ctx.emit_load_const(reg, idx);
        reg
    }

    fn emit_class_method_function(
        &self, method: &oxide_parser::MethodDefinition, method_name: &str, home_reg: u8, ctx: &mut CompileCtx,
        self_binding: &[(&str, u8)],
    ) -> Result<u8, String> {
        let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
        let saved_instance = ctx.in_instance_method;
        let saved_static = ctx.in_static_method;
        ctx.in_instance_method = !method.r#static;
        ctx.in_static_method = method.r#static;
        let mut method_module = self.compile_function_body_with_bindings(
            &param_names,
            body_stmts,
            ctx,
            false,
            self_binding,
            FunctionBodyContext::ClassElement,
        )?;
        ctx.in_instance_method = saved_instance;
        ctx.in_static_method = saved_static;
        method_module.function_name = Some(method_name.to_string());
        method_module.needs_home_object = true;
        ctx.sub_modules.push(method_module);

        let method_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
        let method_reg = ctx.alloc_reg();
        ctx.emit_load_const(method_reg, method_idx);
        ctx.emit(opcode::encode(OpCode::SET_HOME_OBJECT, method_reg, home_reg, 0));
        Ok(method_reg)
    }

    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(es) => Ok(Some(self.emit_expression(&es.expression, ctx)?)),
            Statement::VariableDeclaration(decl) => {
                let mut r = None;
                for d in &decl.declarations {
                    let name = match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                        _ => return Err("destructuring not supported".into()),
                    };
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if is_const && d.init.is_none() {
                        return Err("const declaration must have an initializer".into());
                    }
                    let var_reg = ctx.alloc_reg();
                    ctx.declare(name, var_reg, decl.kind, is_const)?;
                    if let Some(init) = &d.init {
                        let val_reg = self.emit_expression(init, ctx)?;
                        let const_flag = if is_const { 1 } else { 0 };
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, const_flag));
                        ctx.init_var(name);
                        // Name inference (D-04): if the initializer is an arrow function,
                        // set the compiled sub_module's function_name.
                        if matches!(*init, Expression::ArrowFunctionExpression(_)) {
                            if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                                sub_mod.function_name = Some(name.to_string());
                            }
                        }
                        r = Some(val_reg);
                    } else {
                        let idx = ctx.add_constant(Constant::Undefined);
                        let tmp = ctx.alloc_reg();
                        ctx.emit_load_const(tmp, idx);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, tmp, 0));
                        ctx.init_var(name);
                        r = Some(var_reg);
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
                ctx.push_scope();
                let mut r = None;
                for s in &block.body {
                    if let Some(rr) = self.emit_statement(s, ctx)? {
                        r = Some(rr);
                    }
                }
                ctx.pop_scope();
                Ok(r)
            }
            Statement::IfStatement(ifs) => {
                let id = ctx.next_label_id();
                let else_label = Label::IfElse(id);
                let end_label = Label::IfEnd(id);

                let test_reg = self.emit_expression(&ifs.test, ctx)?;

                let else_pos = ctx.resolve_label(else_label)?;
                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                if let Some(r) = cons_reg {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                } else {
                    let undef_idx = ctx.add_constant(Constant::Undefined);
                    ctx.emit_load_const(result_reg, undef_idx);
                }

                let has_alt = ifs.alternate.is_some();
                if has_alt {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp(offset as i16));
                }

                if let Some(alt) = &ifs.alternate {
                    let alt_reg = self.emit_statement(alt, ctx)?;
                    if let Some(r) = alt_reg {
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                    } else {
                        let undef_idx = ctx.add_constant(Constant::Undefined);
                        ctx.emit_load_const(result_reg, undef_idx);
                    }
                }

                Ok(Some(result_reg))
            }
            Statement::WhileStatement(wh) => {
                let id = ctx.next_label_id();
                let start_label = Label::WhileStart(id);
                let end_label = Label::WhileEnd(id);

                ctx.push_loop(end_label, start_label);

                let test_reg = self.emit_expression(&wh.test, ctx)?;

                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                self.emit_statement(&wh.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::DoWhileStatement(dw) => {
                let id = ctx.next_label_id();
                let start_label = Label::DoWhileStart(id);
                let end_label = Label::DoWhileEnd(id);

                ctx.push_loop(end_label, start_label);

                self.emit_statement(&dw.body, ctx)?;

                let test_reg = self.emit_expression(&dw.test, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_true(test_reg, offset as i16));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::ForStatement(fr) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForStart(id);
                let update_label = Label::ForUpdate(id);
                let end_label = Label::ForEnd(id);

                ctx.push_loop(end_label, update_label);

                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.emit_expression(expr, ctx)?;
                    } else if let ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            let name = match &d.id {
                                oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                                _ => return Err("destructuring not supported".into()),
                            };
                            let var_reg = ctx.alloc_reg();
                            ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                            if let Some(init_expr) = &d.init {
                                let val_reg = self.emit_expression(init_expr, ctx)?;
                                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                                ctx.init_var(name);
                            }
                        }
                    }
                }

                if let Some(test) = &fr.test {
                    let test_reg = self.emit_expression(test, ctx)?;
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));
                }

                self.emit_statement(&fr.body, ctx)?;

                if let Some(update) = &fr.update {
                    self.emit_expression(update, ctx)?;
                }

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.pop_loop();

                Ok(None)
            }
            Statement::ForInStatement(fi) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForInStart(id);
                let end_label = Label::ForInEnd(id);

                let obj_reg = self.emit_expression(&fi.right, ctx)?;
                ctx.emit(opcode::encode(OpCode::FOR_IN_INIT, 0, obj_reg, 0));

                ctx.push_loop(end_label, start_label);

                let done_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_DONE, done_reg, 0, 0));

                let end_pos = ctx.resolve_label(end_label)?;
                let cleanup_jmp_offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(done_reg, 2));
                ctx.emit(opcode::encode_jmp(cleanup_jmp_offset as i16));

                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_NEXT, key_reg, 0, 0));

                match &fi.left {
                    ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            let name = match &d.id {
                                oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                                _ => return Err("destructuring not supported".into()),
                            };
                            let var_reg = ctx.alloc_reg();
                            ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                            ctx.init_var(name);
                        }
                    }
                    ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                    }
                    _ => return Err("unsupported for-in left-hand side".into()),
                }

                self.emit_statement(&fi.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.emit(opcode::encode(OpCode::FOR_IN_CLEANUP, 0, 0, 0));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
                let compare_reg_checkpoint = ctx.reg_checkpoint();
                let cases = &sw.cases;

                for (case_idx, case) in cases.iter().enumerate() {
                    if let Some(test) = &case.test {
                        let test_reg = self.emit_expression(test, ctx)?;
                        let eq_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::EQ, eq_reg, disc_reg, test_reg));

                        let case_label = Label::SwitchCase(id, case_idx as u32);
                        let body_pos = ctx.resolve_label(case_label)?;
                        let offset = (body_pos as isize) - (ctx.bytecode.len() as isize);
                        ctx.emit(opcode::encode_jmp_if_true(eq_reg, offset as i16));
                        ctx.restore_reg_checkpoint(compare_reg_checkpoint);
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp(offset as i16));
                }

                for case in cases.iter() {
                    for s in &case.consequent {
                        self.emit_statement(s, ctx)?;
                    }
                }

                ctx.pop_switch();
                Ok(None)
            }
            Statement::BreakStatement(_) => {
                let break_label = if let Some(sw_label) = ctx.current_switch() {
                    *sw_label
                } else {
                    let (bl, _) = ctx.current_loop().ok_or("break outside switch or loop".to_string())?;
                    *bl
                };
                let break_pos = ctx.resolve_label(break_label)?;
                let offset = (break_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));
                Ok(None)
            }
            Statement::ContinueStatement(_) => {
                let (_, continue_label) = ctx.current_loop().ok_or("continue outside loop".to_string())?;
                let continue_pos = ctx.resolve_label(*continue_label)?;
                let offset = (continue_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));
                Ok(None)
            }
            Statement::FunctionDeclaration(fd) => {
                // FunctionDeclaration: emit LOAD_CONST(BytecodeFunc) + STORE_VAR
                let name = if let Some(id) = &fd.id {
                    id.name.to_string()
                } else {
                    return Err("FunctionDeclaration without name".into());
                };

                // Extract params
                let mut param_names = Vec::new();
                for param in &fd.params.items {
                    if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                        param_names.push(bi.name.to_string());
                    }
                }

                // Extract body statements (pass by reference)
                let body_stmts: &[Statement] = if let Some(body) = &fd.body { &body.statements } else { &[] };

                // Compile body into sub-module
                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
                sub_module.function_name = Some(name.clone());
                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                // Use the register allocated during the count/hoist pass. Calling
                // lookup_or_global here would allocate an extra register during
                // emission and desynchronize later function declaration bindings.
                let var_reg = ctx.lookup(&name)?;
                ctx.reserve_reg(var_reg);
                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                ctx.emit_load_const(var_reg, const_idx);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, var_reg, 0));

                Ok(None)
            }
            Statement::ClassDeclaration(class) => {
                let name = class
                    .id
                    .as_ref()
                    .map(|id| id.name.to_string())
                    .ok_or_else(|| "ClassDeclaration without name".to_string())?;
                let var_reg = ctx.alloc_reg();
                ctx.declare(&name, var_reg, VariableDeclarationKind::Let, false)?;
                ctx.init_var(&name);
                let ctor_reg = self.emit_class(class, ctx)?;
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, ctor_reg, 0));
                Ok(None)
            }
            Statement::ThrowStatement(ts) => {
                let exc_reg = self.emit_expression(&ts.argument, ctx)?;
                ctx.emit(opcode::encode(OpCode::THROW, exc_reg, 0, 0));
                Ok(None)
            }
            Statement::TryStatement(ts) => {
                let id = ctx.next_label_id();
                let catch_label = Label::CatchBody(id);
                let try_end_label = Label::TryEnd(id);
                let has_catch = ts.handler.is_some();
                let has_finally = ts.finalizer.is_some();

                let result_reg = ctx.alloc_reg();
                let mut try_finally_begin_pos: Option<usize> = None;
                let mut try_begin_pos: Option<usize> = None;

                if has_finally {
                    try_finally_begin_pos = Some(ctx.bytecode.len());
                    ctx.emit(opcode::encode_try_finally_begin(0)); // placeholder
                }

                if has_catch {
                    try_begin_pos = Some(ctx.bytecode.len());
                    ctx.emit(opcode::encode_try_begin(0)); // placeholder
                }

                let mut last_try_result: Option<u8> = None;
                for s in &ts.block.body {
                    if let Some(r) = self.emit_statement(s, ctx)? {
                        last_try_result = Some(r);
                    }
                }
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_try_result.unwrap_or(result_reg), 0));

                if has_catch {
                    ctx.emit(opcode::encode(OpCode::TRY_END, 0, 0, 0));
                }

                let jmp_needed = has_catch || has_finally;
                let jmp_skip_pos = if jmp_needed {
                    let pos = ctx.bytecode.len();
                    ctx.emit(opcode::encode_jmp(0)); // placeholder
                    Some(pos)
                } else {
                    None
                };

                let catch_label_pc = ctx.bytecode.len();
                ctx.label_map.insert(catch_label, catch_label_pc);

                if let Some(try_begin_pc) = try_begin_pos {
                    let offset = catch_label_pc as isize - (try_begin_pc as isize);
                    ctx.bytecode[try_begin_pc] = opcode::encode_try_begin(offset as i16);
                }

                if let Some(catch) = &ts.handler {
                    ctx.push_scope();
                    if let Some(param) = &catch.param {
                        let catch_reg = ctx.alloc_reg();
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                            ctx.declare_initialized(bi.name.as_str(), catch_reg, VariableDeclarationKind::Let, false)?;
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, catch_reg, 0, 0));
                        }
                    }
                    let mut last_catch_result: Option<u8> = None;
                    for s in &catch.body.body {
                        if let Some(r) = self.emit_statement(s, ctx)? {
                            last_catch_result = Some(r);
                        }
                    }
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_catch_result.unwrap_or(result_reg), 0));
                    ctx.pop_scope();
                }

                if has_finally {
                    let finally_label = Label::FinallyBody(id);
                    let finally_label_pc = ctx.bytecode.len();
                    ctx.label_map.insert(finally_label, finally_label_pc);
                    if let Some(fb_pos) = try_finally_begin_pos {
                        let offset = finally_label_pc as isize - (fb_pos as isize);
                        ctx.bytecode[fb_pos] = opcode::encode_try_finally_begin(offset as i16);
                    }

                    if let Some(jmp_pos) = jmp_skip_pos {
                        let offset = finally_label_pc as isize - (jmp_pos as isize);
                        ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset as i16);
                    }

                    let mut last_finally_result: Option<u8> = None;
                    for s in &ts.finalizer.as_ref().unwrap().body {
                        if let Some(r) = self.emit_statement(s, ctx)? {
                            last_finally_result = Some(r);
                        }
                    }
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_VAR,
                        result_reg,
                        last_finally_result.unwrap_or(result_reg),
                        0,
                    ));
                    ctx.emit(opcode::encode(OpCode::TRY_FINALLY_END, 0, 0, 0));
                } else if let Some(jmp_pos) = jmp_skip_pos {
                    let offset = ctx.bytecode.len() as isize - (jmp_pos as isize);
                    ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset as i16);
                }

                let try_end_pc = ctx.bytecode.len();
                ctx.label_map.insert(try_end_label, try_end_pc);

                Ok(Some(result_reg))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => {
                let idx = if is_int_literal(n.value) {
                    ctx.add_constant(Constant::Int(n.value as i32))
                } else {
                    ctx.add_constant(Constant::Number(n.value))
                };
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::StringLiteral(s) => {
                let idx = ctx.add_constant(Constant::String(s.value.to_string()));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::BooleanLiteral(b) => {
                let idx = ctx.add_constant(Constant::Boolean(b.value));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::NullLiteral(_) => {
                let idx = ctx.add_constant(Constant::Null);
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
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
                    BinaryOperator::BitwiseAnd => OpCode::BIT_AND,
                    BinaryOperator::BitwiseOR => OpCode::BIT_OR,
                    BinaryOperator::BitwiseXOR => OpCode::BIT_XOR,
                    BinaryOperator::ShiftLeft => OpCode::SHL,
                    BinaryOperator::ShiftRight => OpCode::SHR,
                    BinaryOperator::ShiftRightZeroFill => OpCode::USHR,
                    BinaryOperator::Equality => OpCode::EQ,
                    BinaryOperator::Inequality => OpCode::NEQ,
                    BinaryOperator::LessThan => OpCode::LT,
                    BinaryOperator::GreaterThan => OpCode::GT,
                    BinaryOperator::LessEqualThan => OpCode::LTE,
                    BinaryOperator::GreaterEqualThan => OpCode::GTE,
                    BinaryOperator::In => OpCode::IN,
                    BinaryOperator::Instanceof => OpCode::INSTANCEOF,
                    BinaryOperator::StrictEquality => OpCode::STRICT_EQ,
                    BinaryOperator::StrictInequality => OpCode::STRICT_NEQ,
                    _ => return Err(format!("unsupported binary operator: {:?}", bin.operator)),
                };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(op, r, left, right));
                Ok(r)
            }
            Expression::UnaryExpression(un) => {
                let arg = self.emit_expression(&un.argument, ctx)?;
                match un.operator {
                    UnaryOperator::UnaryNegation => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::NEG, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::Typeof => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::TYPEOF, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::Void => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::VOID, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::LogicalNot => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::NOT, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::BitwiseNot => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::BIT_NOT, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::UnaryPlus => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::UNARY_PLUS, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::Delete => match &un.argument {
                        Expression::Identifier(_) => {
                            Err("SyntaxError: delete of an unqualified identifier in strict mode".into())
                        }
                        Expression::StaticMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            let prop_name = member.property.name.as_str();
                            let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                            let r = ctx.alloc_reg();
                            ctx.emit(opcode::encode(OpCode::DELETE_PROP_STATIC, r, obj_reg, 0));
                            ctx.emit(const_idx as u32);
                            Ok(r)
                        }
                        Expression::ComputedMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            let key_reg = self.emit_expression(&member.expression, ctx)?;
                            let r = ctx.alloc_reg();
                            ctx.emit(opcode::encode(OpCode::DELETE_PROP_DYNAMIC, r, obj_reg, key_reg));
                            Ok(r)
                        }
                        _ => Err("invalid delete target".into()),
                    },
                }
            }
            Expression::ConditionalExpression(cond) => {
                let id = ctx.next_label_id();
                let else_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);

                let test_reg = self.emit_expression(&cond.test, ctx)?;
                let else_pos = ctx.resolve_label(else_label)?;
                let end_pos = ctx.resolve_label(end_label)?;

                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                let cons_reg = self.emit_expression(&cond.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, cons_reg, 0));

                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                let alt_reg = self.emit_expression(&cond.alternate, ctx)?;
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, alt_reg, 0));

                Ok(result_reg)
            }
            Expression::LogicalExpression(log) => {
                use oxide_parser::LogicalOperator;
                let left_reg = self.emit_expression(&log.left, ctx)?;

                if is_side_effect_free(&log.left) && is_side_effect_free(&log.right) {
                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    let r = ctx.alloc_reg();
                    let op = match log.operator {
                        LogicalOperator::And => OpCode::AND,
                        LogicalOperator::Or => OpCode::OR,
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    };
                    ctx.emit(opcode::encode(op, r, left_reg, right_reg));
                    Ok(r)
                } else {
                    let id = ctx.next_label_id();
                    let skip_label = match log.operator {
                        LogicalOperator::And => Label::TernaryEnd(id),
                        LogicalOperator::Or => Label::TernaryElse(id),
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    };
                    let skip_pos = ctx.resolve_label(skip_label)?;

                    let dup_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, left_reg, 0));

                    let offset = (skip_pos as isize) - (ctx.bytecode.len() as isize);
                    match log.operator {
                        LogicalOperator::And => {
                            ctx.emit(opcode::encode_jmp_if_false(dup_reg, offset as i16));
                        }
                        LogicalOperator::Or => {
                            ctx.emit(opcode::encode_jmp_if_true(dup_reg, offset as i16));
                        }
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    }

                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, right_reg, 0));

                    Ok(dup_reg)
                }
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let prop_name = member.property.name.as_str();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    let this_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
                    let result_reg = ctx.alloc_reg();
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.emit(opcode::encode(op, result_reg, this_reg, key_reg));
                    return Ok(result_reg);
                }
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_CONST, key_reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
                ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, obj_reg, key_reg));
                ctx.emit(0);
                ctx.emit(0);
                ctx.emit(0);
                Ok(obj_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, obj_reg, key_reg, r));
                Ok(r)
            }
            Expression::ObjectExpression(obj) => {
                let obj_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_OBJECT, obj_reg, 0, 0));
                for prop in &obj.properties {
                    let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop else {
                        return Err("spread properties not yet supported".into());
                    };
                    let prop_name = match &p.key {
                        oxide_parser::PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                        oxide_parser::PropertyKey::StringLiteral(s) => s.value.to_string(),
                        _ => return Err("unsupported object property key type".into()),
                    };
                    if matches!(p.kind, PropertyKind::Get | PropertyKind::Set) {
                        let accessor_reg = self.emit_expression(&p.value, ctx)?;
                        if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                        let undef_reg = self.emit_undefined(ctx);
                        let (get_reg, set_reg) = match p.kind {
                            PropertyKind::Get => (accessor_reg, undef_reg),
                            PropertyKind::Set => (undef_reg, accessor_reg),
                            _ => unreachable!(),
                        };
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, obj_reg, get_reg, set_reg));
                        ctx.emit(idx as u32);
                    } else {
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit_load_const(key_reg, idx);
                        let val_reg = self.emit_expression(&p.value, ctx)?;
                        // Name inference (D-04): if property value is an arrow function,
                        // set the compiled sub_module's function_name to the property key.
                        if matches!(&p.value, Expression::ArrowFunctionExpression(_)) {
                            if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                                sub_mod.function_name = Some(prop_name.to_string());
                            }
                        }
                        ctx.emit(opcode::encode(OpCode::SET_PROP, obj_reg, val_reg, key_reg));
                    }
                }
                Ok(obj_reg)
            }
            Expression::ArrayExpression(arr) => {
                let arr_reg = ctx.alloc_reg();
                let n = arr.elements.len() as u16;
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, arr_reg, (n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8));
                for (i, elem) in arr.elements.iter().enumerate() {
                    let Some(e) = elem.as_expression() else {
                        return Err("spread not supported".into());
                    };
                    let val_reg = self.emit_expression(e, ctx)?;
                    let idx_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::Int(i as i32));
                    ctx.emit_load_const(idx_reg, idx);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, arr_reg, idx_reg, val_reg));
                }
                Ok(arr_reg)
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    if assign.operator != AssignmentOperator::Assign {
                        let op = match assign.operator {
                            AssignmentOperator::Addition => OpCode::COMPOUND_MEMBER_ADD,
                            AssignmentOperator::Subtraction => OpCode::COMPOUND_MEMBER_SUB,
                            AssignmentOperator::Multiplication => OpCode::COMPOUND_MEMBER_MUL,
                            AssignmentOperator::Division => OpCode::COMPOUND_MEMBER_DIV,
                            AssignmentOperator::Remainder => OpCode::COMPOUND_MEMBER_MOD,
                            AssignmentOperator::Exponential => OpCode::COMPOUND_MEMBER_EXP,
                            _ => {
                                return Err(format!("compound assignment operator {:?} not supported", assign.operator))
                            }
                        };
                        ctx.emit(opcode::encode(op, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    } else {
                        ctx.emit(opcode::encode(OpCode::IC_SET_PROP, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    }
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, obj_reg, key_reg, val_reg));
                    Ok(val_reg)
                } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) = &assign.left {
                    if assign.operator != AssignmentOperator::Assign {
                        if assign.operator == AssignmentOperator::Addition
                            || assign.operator == AssignmentOperator::Subtraction
                            || assign.operator == AssignmentOperator::Multiplication
                            || assign.operator == AssignmentOperator::Division
                            || assign.operator == AssignmentOperator::Remainder
                            || assign.operator == AssignmentOperator::Exponential
                            || assign.operator == AssignmentOperator::BitwiseAnd
                            || assign.operator == AssignmentOperator::BitwiseOR
                            || assign.operator == AssignmentOperator::BitwiseXOR
                            || assign.operator == AssignmentOperator::ShiftLeft
                            || assign.operator == AssignmentOperator::ShiftRight
                            || assign.operator == AssignmentOperator::ShiftRightZeroFill
                        {
                            let rhs = self.emit_expression(&assign.right, ctx)?;
                            let name = id_ref.name.as_str();
                            let var_reg = ctx.lookup_or_global(name);
                            let op = match assign.operator {
                                AssignmentOperator::Addition => OpCode::COMPOUND_ADD,
                                AssignmentOperator::Subtraction => OpCode::COMPOUND_SUB,
                                AssignmentOperator::Multiplication => OpCode::COMPOUND_MUL,
                                AssignmentOperator::Division => OpCode::COMPOUND_DIV,
                                AssignmentOperator::Remainder => OpCode::COMPOUND_MOD,
                                AssignmentOperator::Exponential => OpCode::COMPOUND_EXP,
                                AssignmentOperator::BitwiseAnd => OpCode::COMPOUND_AND,
                                AssignmentOperator::BitwiseOR => OpCode::COMPOUND_OR,
                                AssignmentOperator::BitwiseXOR => OpCode::COMPOUND_XOR,
                                AssignmentOperator::ShiftLeft => OpCode::COMPOUND_SHL,
                                AssignmentOperator::ShiftRight => OpCode::COMPOUND_SHR,
                                AssignmentOperator::ShiftRightZeroFill => OpCode::COMPOUND_USHR,
                                _ => {
                                    return Err(format!(
                                        "compound assignment operator {:?} not supported",
                                        assign.operator
                                    ))
                                }
                            };
                            ctx.emit(opcode::encode(op, var_reg, rhs, 0));
                            Ok(var_reg)
                        } else {
                            Err(format!("compound assignment operator {:?} not supported", assign.operator))
                        }
                    } else {
                        let val_reg = self.emit_expression(&assign.right, ctx)?;
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        let is_const = ctx.lookup_const_flag(name);
                        let const_flag = if is_const { 1 } else { 0 };
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, const_flag));
                        Ok(val_reg)
                    }
                } else {
                    Err("assignment target not supported".into())
                }
            }
            Expression::UpdateExpression(update) => match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                    let name = id.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    let result_reg = ctx.alloc_reg();
                    let op = match (update.operator, update.prefix) {
                        (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                        (UpdateOperator::Increment, false) => OpCode::INC_POST,
                        (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                        (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
                    };
                    ctx.emit(opcode::encode(op, var_reg, result_reg, result_reg));
                    Ok(result_reg)
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let key_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        key_reg,
                        (key_idx & 0xFF) as u8,
                        ((key_idx >> 8) & 0xFF) as u8,
                    ));
                    let val_reg = ctx.alloc_reg();
                    let op = match update.operator {
                        UpdateOperator::Increment => OpCode::MEMBER_INC,
                        UpdateOperator::Decrement => OpCode::MEMBER_DEC,
                    };
                    ctx.emit(opcode::encode(op, obj_reg, val_reg, key_reg));
                    ctx.emit(0);
                    ctx.emit(0);
                    ctx.emit(0);
                    Ok(val_reg)
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let val_reg = ctx.alloc_reg();
                    let op = match update.operator {
                        UpdateOperator::Increment => OpCode::DYN_MEMBER_INC,
                        UpdateOperator::Decrement => OpCode::DYN_MEMBER_DEC,
                    };
                    ctx.emit(opcode::encode(op, obj_reg, key_reg, val_reg));
                    Ok(val_reg)
                }
                _ => Err("member update not yet supported".into()),
            },
            Expression::Identifier(ident) => {
                let var_reg = ctx.lookup(ident.name.as_str())?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
                Ok(r)
            }
            // TemplateLiteral: interleaved quasis + expressions via TEMPLATE_STR opcode (D-07)
            Expression::TemplateLiteral(tl) => {
                let r = ctx.alloc_reg();
                let quasis = &tl.quasis;
                let expressions = &tl.expressions;
                let segment_count = quasis.len() + expressions.len();

                // Evaluate each expression, collecting registers
                let expr_regs: Vec<u8> = expressions
                    .iter()
                    .map(|e| self.emit_expression(e, ctx))
                    .collect::<Result<Vec<_>, _>>()?;

                // Add each quasi string to constant pool
                let quasi_const_idxs: Vec<u16> = quasis
                    .iter()
                    .map(|q| {
                        let s = q.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                        ctx.add_constant(Constant::String(s))
                    })
                    .collect();

                // Compute total length hint
                let total_len_hint: usize = quasis
                    .iter()
                    .map(|q| q.value.cooked.as_ref().map(|c| c.len()).unwrap_or(0))
                    .sum();

                // Emit TEMPLATE_STR rd, 0, 0
                ctx.emit(opcode::encode(OpCode::TEMPLATE_STR, r, 0, 0));

                // Ext word 0: (segment_count << 16) | (total_len_hint & 0xFFFF)
                ctx.emit(((segment_count as u32) << 16) | (total_len_hint as u32 & 0xFFFF));

                // Interleave segments: quasi[0], expr[0], quasi[1], expr[1], ...
                let mut expr_iter = expr_regs.iter();
                for const_idx in quasi_const_idxs.iter() {
                    // Quasi: is_expression=0, bits 0-15 = const_idx
                    ctx.emit(*const_idx as u32 & 0x7FFF_FFFF);

                    // Expression (if any remaining)
                    if let Some(expr_reg) = expr_iter.next() {
                        // Expression: is_expression=1, bits 0-15 = reg
                        ctx.emit(0x8000_0000u32 | (*expr_reg as u32));
                    }
                }

                Ok(r)
            }
            // TaggedTemplateExpression: tag`str ${expr}` => CALL(tag, undefined, cooked_array, raw_array, ...exprs) (D-08)
            Expression::TaggedTemplateExpression(tt) => {
                let quasis = &tt.quasi.quasis;
                let expressions = &tt.quasi.expressions;

                // 1. Evaluate tag expression
                let tag_reg = self.emit_expression(&tt.tag, ctx)?;

                // 2. Build cooked strings array (into a temp register)
                let cooked_temp = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, cooked_temp, quasis.len() as u8, 0));
                for (i, quasi) in quasis.iter().enumerate() {
                    let s = quasi.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                    let const_idx = ctx.add_constant(Constant::String(s));
                    let str_reg = ctx.alloc_reg();
                    ctx.emit_load_const(str_reg, const_idx);
                    let idx_const = ctx.add_constant(Constant::Int(i as i32));
                    let idx_reg = ctx.alloc_reg();
                    ctx.emit_load_const(idx_reg, idx_const);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, cooked_temp, idx_reg, str_reg));
                }

                // 3. Build raw strings array (into a temp register)
                let raw_temp = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, raw_temp, quasis.len() as u8, 0));
                for (i, quasi) in quasis.iter().enumerate() {
                    let raw = quasi.value.raw.to_string();
                    let const_idx = ctx.add_constant(Constant::String(raw));
                    let str_reg = ctx.alloc_reg();
                    ctx.emit_load_const(str_reg, const_idx);
                    let idx_const = ctx.add_constant(Constant::Int(i as i32));
                    let idx_reg = ctx.alloc_reg();
                    ctx.emit_load_const(idx_reg, idx_const);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, raw_temp, idx_reg, str_reg));
                }

                // 4. Evaluate expression arguments (into temp registers)
                let mut expr_temps = Vec::new();
                for expr in expressions {
                    expr_temps.push(self.emit_expression(expr, ctx)?);
                }

                // 5. Allocate consecutive argument slots: cooked, raw, expr[0], expr[1], ...
                let cooked_slot = ctx.alloc_reg();
                let raw_slot = ctx.alloc_reg();
                let mut expr_slots = Vec::new();
                for _ in expressions {
                    expr_slots.push(ctx.alloc_reg());
                }

                // Copy temps to consecutive slots using LOAD_VAR (register-to-register move)
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, cooked_slot, cooked_temp, 0));
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, raw_slot, raw_temp, 0));
                for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, *slot, *temp, 0));
                }

                // 6. Emit undefined as this_arg
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.emit_load_const(undef_reg, undef_idx);

                // 7. Emit CALL(tag, undefined, cooked_slot)
                let arg_count = 2 + expressions.len();
                ctx.emit(opcode::encode(OpCode::CALL, tag_reg, undef_reg, cooked_slot));
                ctx.emit(arg_count as u32);

                // 8. Result from regs[0]
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
                Ok(result_reg)
            }
            // ArrowFunctionExpression: compile body, capture lexical this (D-01)
            // Name inference (D-04) happens at assignment site - see VariableDeclaration/ObjectProperty.
            Expression::ArrowFunctionExpression(arrow) => {
                // Rest params not yet supported (D-06 placeholder)
                if let Some(_rest) = &arrow.params.rest {
                    return Err("rest params in arrow functions not yet supported".into());
                }

                // Extract param names (same pattern as FunctionExpression)
                let mut param_names = Vec::new();
                for param in &arrow.params.items {
                    if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                        param_names.push(bi.name.to_string());
                    }
                }

                // Expression body: pass body statements directly with is_expression_body=true.
                // Statement body: pass body statements with is_expression_body=false.
                let body_stmts = &arrow.body.statements;
                let is_expr_body = arrow.expression;

                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, is_expr_body, true)?;
                sub_module.is_arrow = true;

                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, const_idx);
                Ok(r)
            }
            Expression::FunctionExpression(fe) => {
                // FunctionExpression: compile body, emit LOAD_CONST(BytecodeFunc)
                let mut param_names = Vec::new();
                for param in &fe.params.items {
                    if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                        param_names.push(bi.name.to_string());
                    }
                }

                let body_stmts: &[Statement] = if let Some(body) = &fe.body { &body.statements } else { &[] };

                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
                if let Some(id) = &fe.id {
                    sub_module.function_name = Some(id.name.to_string());
                }
                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, const_idx);
                Ok(r)
            }
            Expression::ClassExpression(class) => self.emit_class(class, ctx),
            Expression::NewExpression(ne) => {
                let constructor_reg = self.emit_expression(&ne.callee, ctx)?;
                let mut arg_regs = Vec::new();
                for arg in &ne.arguments {
                    if let Some(expr) = arg.as_expression() {
                        arg_regs.push(self.emit_expression(expr, ctx)?);
                    }
                }
                let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_EXPRESSION, r, constructor_reg, first_arg_reg));
                ctx.emit(arg_regs.len() as u32);
                Ok(r)
            }
            Expression::ParenthesizedExpression(p) => self.emit_expression(&p.expression, ctx),
            Expression::CallExpression(call) => {
                if matches!(&call.callee, Expression::Super(_)) {
                    if !ctx.in_derived_constructor {
                        return Err("super() only supported in derived constructors".into());
                    }
                    let mut arg_regs = Vec::new();
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            arg_regs.push(self.emit_expression(expr, ctx)?);
                        }
                    }
                    let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
                    let result_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::SUPER_CALL, result_reg, first_arg_reg, 0));
                    ctx.emit(arg_regs.len() as u32);
                    return Ok(result_reg);
                }
                let (callee_reg, this_reg) = match &call.callee {
                    Expression::StaticMemberExpression(member) => {
                        let is_super_member = matches!(&member.object, Expression::Super(_));
                        let obj_reg = if is_super_member {
                            let this_reg = ctx.alloc_reg();
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
                            this_reg
                        } else {
                            self.emit_expression(&member.object, ctx)?
                        };
                        let prop_name = member.property.name.as_str();
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit_load_const(key_reg, idx);
                        let callee_reg = ctx.alloc_reg();
                        if is_super_member {
                            if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                                return Err("super property only supported in class methods".into());
                            }
                            let op = if ctx.in_static_method {
                                OpCode::SUPER_STATIC_GET_PROP
                            } else {
                                OpCode::SUPER_GET_PROP
                            };
                            ctx.emit(opcode::encode(op, callee_reg, obj_reg, key_reg));
                        } else {
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, callee_reg, obj_reg, 0));
                            ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, callee_reg, key_reg));
                            ctx.emit(0);
                            ctx.emit(0);
                            ctx.emit(0);
                        }
                        (callee_reg, obj_reg)
                    }
                    _ => {
                        let callee_reg = self.emit_expression(&call.callee, ctx)?;
                        let this_idx = ctx.add_constant(Constant::Undefined);
                        let this_reg = ctx.alloc_reg();
                        ctx.emit_load_const(this_reg, this_idx);
                        (callee_reg, this_reg)
                    }
                };
                let mut arg_regs = Vec::new();
                for arg in &call.arguments {
                    if let Some(expr) = arg.as_expression() {
                        arg_regs.push(self.emit_expression(expr, ctx)?);
                    }
                }
                let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
                let op = match &call.callee {
                    Expression::Identifier(ident) if ctx.is_builtin(ident.name.as_str()) => OpCode::CALL_NATIVE,
                    Expression::StaticMemberExpression(m) => {
                        if let Expression::Identifier(ident) = &m.object {
                            if ctx.is_builtin(ident.name.as_str()) {
                                OpCode::CALL_NATIVE
                            } else {
                                OpCode::CALL
                            }
                        } else {
                            OpCode::CALL
                        }
                    }
                    _ => OpCode::CALL,
                };
                ctx.emit(opcode::encode(op, callee_reg, this_reg, first_arg_reg));
                ctx.emit(arg_regs.len() as u32);
                // Copy result from regs[0] into a dedicated register so multiple
                // call expressions don't overwrite each other.
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
                Ok(result_reg)
            }
            Expression::ThisExpression(_) => {
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, 254, 0));
                Ok(r)
            }
            Expression::RegExpLiteral(lit) => {
                if let Some(raw) = &lit.raw {
                    let raw_str = raw.to_string();
                    if raw_str.len() >= 2 && raw_str.starts_with('/') {
                        let last_slash = raw_str.rfind('/').unwrap_or(raw_str.len() - 1);
                        let pattern = &raw_str[1..last_slash];
                        let flags = &raw_str[last_slash + 1..];
                        let ci = ctx.add_constant(Constant::RegExp(pattern.to_string(), flags.to_string()));
                        let r = ctx.alloc_reg();
                        ctx.emit_load_const(r, ci);
                        Ok(r)
                    } else {
                        Err(format!("unsupported expression type: {:?}", expr))
                    }
                } else {
                    Err(format!("unsupported expression type: {:?}", expr))
                }
            }
            _ => Err(format!("unsupported expression type: {:?}", expr)),
        }
    }
}
