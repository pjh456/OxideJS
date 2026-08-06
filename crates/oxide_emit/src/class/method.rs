//! 类方法 emit：`emit_class_methods` 逐方法构造，`emit_class_method_function`
//! 负责单个方法函数体（含 `super` 与 home object）。

use crate::{CompileCtx, Emitter, FunctionBodyContext};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{ClassElement, MethodDefinitionKind, PropertyKey};

impl Emitter {
    pub(crate) fn emit_class_methods(
        &self, elements: &[ClassElement], ctor_reg: u32, proto_reg: u32, self_binding: &[(&str, u32)],
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        for element in elements {
            if let ClassElement::MethodDefinition(method) = element {
                let method = method.as_ref();
                if method.kind == MethodDefinitionKind::Constructor {
                    continue;
                }
                if matches!(method.key, PropertyKey::PrivateIdentifier(_)) {
                    let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                    self.emit_private_method_init(Operand::Reg(home_reg), method, Operand::Reg(home_reg), ctx)?;
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
                    self.emit_class_method_function(method, &method_name, Operand::Reg(home_reg), ctx, self_binding)?;
                match method.kind {
                    MethodDefinitionKind::Method => {
                        if method.computed {
                            ctx.inst(Inst::new(OpCode::SET_PROP_DYNAMIC, Operand::Reg(home_reg), Operand::Reg(key_reg), Operand::Reg(accessor_reg)));
                        } else {
                            ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(home_reg), Operand::Reg(accessor_reg), Operand::Reg(key_reg)));
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
                        ctx.inst(Inst::define_accessor(
                            Operand::Reg(home_reg),
                            Operand::Reg(get_reg),
                            Operand::Reg(set_reg),
                            key_idx as u32,
                        ));
                    }
                    MethodDefinitionKind::Constructor => continue,
                }
            }
        }
        Ok(())
    }

    pub(crate) fn emit_class_method_function(
        &self, method: &oxide_parser::MethodDefinition, method_name: &str, home_reg: Operand, ctx: &mut CompileCtx,
        self_binding: &[(&str, u32)],
    ) -> Result<u32, String> {
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
        ctx.nested.push(method_module);
        let method_reg = ctx.alloc_reg();
        ctx.inst(Inst::create_closure(Operand::Reg(method_reg), ctx.nested.len() as u16));
        ctx.inst(Inst::new(OpCode::SET_HOME_OBJECT, Operand::Reg(method_reg), home_reg, Operand::None));
        Ok(method_reg)
    }
}
