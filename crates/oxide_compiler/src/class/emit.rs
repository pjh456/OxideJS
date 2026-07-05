use crate::compiler::{CompileCtx, Compiler, FunctionBodyContext};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{Class, ClassElement, MethodDefinitionKind, PropertyKey};

impl Compiler {
    pub(crate) fn emit_class(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u8, String> {
        let mut constructor_method = None;
        let mut instance_fields = Vec::new();
        let mut private_names = Vec::<(String, u32)>::new();
        let is_derived = class.super_class.is_some();

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if let PropertyKey::PrivateIdentifier(private) = &method.key {
                        let name = private.name.as_str().to_string();
                        if private_names.iter().any(|(existing, _)| existing == &name) {
                            return Err(format!("duplicate private name #{name}"));
                        }
                        let id = ctx.scopes.next_private_name_id;
                        ctx.scopes.next_private_name_id = ctx.scopes.next_private_name_id.saturating_add(1);
                        private_names.push((name, id));
                    }
                    if method.kind == MethodDefinitionKind::Constructor {
                        if constructor_method.is_some() {
                            return Err("duplicate class constructor".into());
                        }
                        constructor_method = Some(method);
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                        let name = private.name.as_str().to_string();
                        if private_names.iter().any(|(existing, _)| existing == &name) {
                            return Err(format!("duplicate private name #{name}"));
                        }
                        let id = ctx.scopes.next_private_name_id;
                        ctx.scopes.next_private_name_id = ctx.scopes.next_private_name_id.saturating_add(1);
                        private_names.push((name, id));
                    }
                    if !prop.r#static {
                        instance_fields.push(prop);
                    }
                }
                ClassElement::AccessorProperty(_) => return Err("class accessor properties not yet supported".into()),
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => {}
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
        let saved_private_names = ctx.scopes.private_name_map.clone();
        ctx.in_derived_constructor = is_derived;
        ctx.scopes.private_name_map = private_names.clone();

        let mut ctor_module = if let Some(method) = constructor_method {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            self.compile_function_body_with_field_hooks(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| {
                    for field in &instance_fields {
                        if matches!(field.key, PropertyKey::PrivateIdentifier(_)) {
                            compiler.count_private_field_init(field.value.as_ref(), field_ctx);
                        } else {
                            compiler.count_public_field_init(
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            );
                        }
                    }
                }),
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| -> Result<(), String> {
                    for field in &instance_fields {
                        if let PropertyKey::PrivateIdentifier(private) = &field.key {
                            compiler.emit_private_field_init(
                                254,
                                private.name.as_str(),
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        } else {
                            compiler.emit_public_field_init(
                                254,
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        }
                    }
                    Ok(())
                }),
                is_derived,
            )?
        } else {
            let mut module = self.compile_function_body_with_field_hooks(
                &[],
                &[],
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| {
                    for field in &instance_fields {
                        if matches!(field.key, PropertyKey::PrivateIdentifier(_)) {
                            compiler.count_private_field_init(field.value.as_ref(), field_ctx);
                        } else {
                            compiler.count_public_field_init(
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            );
                        }
                    }
                }),
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| -> Result<(), String> {
                    for field in &instance_fields {
                        if let PropertyKey::PrivateIdentifier(private) = &field.key {
                            compiler.emit_private_field_init(
                                254,
                                private.name.as_str(),
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        } else {
                            compiler.emit_public_field_init(
                                254,
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        }
                    }
                    Ok(())
                }),
                is_derived,
            )?;
            if is_derived {
                module.bytecode.clear();
                module.constants.clear();
                module.n_registers = 1;
                module.bytecode.push(opcode::encode(OpCode::SUPER_CALL, 0, 0, 0));
                module.bytecode.push(0);
                let mut field_ctx = CompileCtx::new();
                field_ctx.scopes.private_name_map = private_names.clone();
                for field in &instance_fields {
                    if let PropertyKey::PrivateIdentifier(private) = &field.key {
                        self.emit_private_field_init(254, private.name.as_str(), field.value.as_ref(), &mut field_ctx)?;
                    } else {
                        self.emit_public_field_init(
                            254,
                            &field.key,
                            field.computed,
                            field.value.as_ref(),
                            &mut field_ctx,
                        )?;
                    }
                }
                module.bytecode.extend(field_ctx.bytecode);
                module.constants = field_ctx.constants;
                module.n_registers = field_ctx.max_regs.max(1);
                module.bytecode.push(opcode::encode(OpCode::RETURN, 0, 0, 0));
            }
            module
        };
        ctx.in_derived_constructor = saved_derived;
        ctor_module.is_class_constructor = true;
        ctor_module.is_derived_constructor = is_derived;
        ctor_module.function_name = ctor_name.clone();
        ctx.sub_modules.push(ctor_module);

        ctx.emit_create_closure(ctor_reg, ctx.sub_modules.len() as u32);
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

        let ctor_key_idx = ctx.add_constant(Constant::String("constructor".to_string()));
        let ctor_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(ctor_key_reg, ctor_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, ctor_reg, ctor_key_reg));

        let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
        let proto_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(proto_key_reg, proto_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, proto_reg, proto_key_reg));

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if method.kind == MethodDefinitionKind::Constructor {
                        continue;
                    }
                    if matches!(method.key, PropertyKey::PrivateIdentifier(_)) {
                        let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                        self.emit_private_method_init(home_reg, method, home_reg, ctx)?;
                        continue;
                    }
                    let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                    let key_reg = self.emit_class_key_reg(&method.key, method.computed, ctx)?;
                    let method_name = if method.computed {
                        "<computed>".to_string()
                    } else {
                        self.class_property_name(&method.key)?
                    };
                    let accessor_reg =
                        self.emit_class_method_function(method, &method_name, home_reg, ctx, &self_binding)?;
                    match method.kind {
                        MethodDefinitionKind::Method => {
                            if method.computed {
                                ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, home_reg, key_reg, accessor_reg));
                            } else {
                                ctx.emit(opcode::encode(OpCode::SET_PROP, home_reg, accessor_reg, key_reg));
                            }
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            if method.computed {
                                return Err("computed class accessors not yet supported".into());
                            }
                            let undef_reg = self.emit_undefined(ctx);
                            let (get_reg, set_reg) = if method.kind == MethodDefinitionKind::Get {
                                (accessor_reg, undef_reg)
                            } else {
                                (undef_reg, accessor_reg)
                            };
                            let key_name = self.class_property_name(&method.key)?;
                            let key_idx = ctx.add_constant(Constant::String(key_name));
                            ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, home_reg, get_reg, set_reg));
                            ctx.emit(key_idx as u32);
                        }
                        MethodDefinitionKind::Constructor => continue,
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        let saved_static_this = ctx.static_block_this_reg;
                        ctx.static_block_this_reg = Some(ctor_reg);
                        if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                            self.emit_private_field_init(ctor_reg, private.name.as_str(), prop.value.as_ref(), ctx)?;
                        } else {
                            self.emit_public_field_init(ctor_reg, &prop.key, prop.computed, prop.value.as_ref(), ctx)?;
                        }
                        ctx.static_block_this_reg = saved_static_this;
                    }
                }
                ClassElement::StaticBlock(block) => {
                    let saved_static_this = ctx.static_block_this_reg;
                    ctx.static_block_this_reg = Some(ctor_reg);
                    ctx.push_scope();
                    for stmt in &block.body {
                        self.emit_statement(stmt, ctx)?;
                    }
                    ctx.pop_scope();
                    ctx.static_block_this_reg = saved_static_this;
                }
                ClassElement::AccessorProperty(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }

        ctx.scopes.private_name_map = saved_private_names;
        Ok(ctor_reg)
    }

    pub(crate) fn emit_class_method_function(
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

        let method_reg = ctx.alloc_reg();
        ctx.emit_create_closure(method_reg, ctx.sub_modules.len() as u32);
        ctx.emit(opcode::encode(OpCode::SET_HOME_OBJECT, method_reg, home_reg, 0));
        Ok(method_reg)
    }
}
