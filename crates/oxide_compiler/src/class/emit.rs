use crate::compiler::{CompileCtx, Compiler, FunctionBodyContext};
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

        let (ctor_reg, proto_reg, super_reg) = self.emit_class_header(class, ctx)?;
        let ctor_name = class.id.as_ref().map(|id| id.name.to_string());
        let self_binding = ctor_name.as_deref().map(|name| vec![(name, ctor_reg)]).unwrap_or_default();
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
                None::<fn(&Compiler, &mut CompileCtx)>,
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
                None::<fn(&Compiler, &mut CompileCtx)>,
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

        self.emit_class_prototype(ctor_reg, proto_reg, super_reg, ctx.sub_modules.len() as u32, ctx)?;
        self.emit_class_methods(&class.body.body, ctor_reg, proto_reg, &self_binding, ctx)?;
        self.emit_class_static_fields(&class.body.body, ctor_reg, ctx)?;
        self.emit_class_static_blocks(&class.body.body, ctor_reg, ctx)?;

        ctx.scopes.private_name_map = saved_private_names;
        Ok(ctor_reg)
    }
}
