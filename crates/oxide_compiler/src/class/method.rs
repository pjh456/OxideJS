use crate::compiler::{CompileCtx, Compiler, FunctionBodyContext};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{ClassElement, MethodDefinitionKind, PropertyKey};

impl Compiler {
    pub(crate) fn emit_class_methods(
        &self, elements: &[ClassElement], ctor_reg: u8, proto_reg: u8, self_binding: &[(&str, u8)],
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        for element in elements {
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
                        self.emit_class_method_function(method, &method_name, home_reg, ctx, self_binding)?;
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
                _ => {}
            }
        }
        Ok(())
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
