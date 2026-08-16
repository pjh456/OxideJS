//! 类方法 emit：`emit_class_methods` 逐方法构造，`emit_class_method_function`
//! 负责单个方法函数体（含 `super` 与 home object）。

use crate::{CompileCtx, Emitter, FunctionBodyContext};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ClassElement, MethodDefinitionKind, PropertyKey};

/// class 方法/访问器属性描述符：writable + configurable，enumerable = false（规范 DefineMethod）。
const CLASS_METHOD_ATTRS: u32 = 0b101;

impl Emitter {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_class_methods(
        &self, elements: &[ClassElement], ctor_reg: u32, proto_reg: u32, self_binding: &[(&str, u32)],
        class_self_cell: Option<u8>, key_slots: &[Option<u8>], ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        for (element, &key_slot) in elements.iter().zip(key_slots) {
            if let ClassElement::MethodDefinition(method) = element {
                let method = method.as_ref();
                if method.kind == MethodDefinitionKind::Constructor {
                    continue;
                }
                if matches!(method.key, PropertyKey::PrivateIdentifier(_)) {
                    let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                    self.emit_private_method_init(Operand::Reg(home_reg), method, Operand::Reg(home_reg), self_binding, class_self_cell, ctx)?;
                    continue;
                }
                let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                let key_reg = if method.computed {
                    self.emit_class_array_key_reg(key_slot.unwrap_or(0), ctx)?
                } else {
                    self.emit_class_key_reg(&method.key, false, ctx)?
                };
                let method_name = if method.computed {
                    "<computed>".to_string()
                } else {
                    self.class_property_name(&method.key)?
                };
                let accessor_reg =
                    self.emit_class_method_function(method, &method_name, Operand::Reg(home_reg), ctx, self_binding, class_self_cell)?;
                match method.kind {
                    MethodDefinitionKind::Method => {
                        // class 方法按规范为非枚举数据属性（DefineMethod：writable/configurable，enumerable=false）。
                        ctx.inst(Inst::define_prop_attrs(
                            Operand::Reg(home_reg),
                            Operand::Reg(accessor_reg),
                            Operand::Reg(key_reg),
                            CLASS_METHOD_ATTRS,
                        ));
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
                        ctx.inst(Inst::define_accessor_attrs(
                            Operand::Reg(home_reg),
                            Operand::Reg(get_reg),
                            Operand::Reg(set_reg),
                            key_idx as u32,
                            CLASS_METHOD_ATTRS,
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
        self_binding: &[(&str, u32)], class_self_cell: Option<u8>,
    ) -> Result<u32, String> {
        let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
        let saved_instance = ctx.in_instance_method;
        let saved_static = ctx.in_static_method;
        ctx.in_instance_method = !method.r#static;
        ctx.in_static_method = method.r#static;
        // 私有方法/访问器访问需对接收者做 brand 检查：方法函数捕获类 brand 对象
        // （@@class_brand upvalue，值 = 类原型）。
        let mut extra_uv: Vec<(&str, u8)> = ctx
            .captured_bindings
            .get("@@class_brand")
            .map(|c| ("@@class_brand", *c))
            .into_iter()
            .collect();
        if let (Some((name, _)), Some(c)) = (self_binding.first(), class_self_cell) {
            extra_uv.push((name, c));
        }
        let method_value = method.value.as_ref();
        // 生成器/异步/异步生成器方法与普通方法统一走带标志入口：`*m` 置 is_generator，
        // `async *m` 同时置 is_generator 与 is_async，VM 据此按生成器协议创建对象并
        // 处理 body 内 SUSPEND/YIELD/异常边界。
        let mut method_module = self.compile_function_body_with_field_hooks_gen(
            &param_names,
            body_stmts,
            ctx,
            false,
            self_binding,
            FunctionBodyContext::ClassElement,
            None::<fn(&Emitter, &mut CompileCtx) -> Result<(), String>>,
            false,
            &[],
            &extra_uv,
            method_value.generator,
            method_value.r#async,
        )?;
        ctx.in_instance_method = saved_instance;
        ctx.in_static_method = saved_static;
        // 访问器函数名带 "get "/"set " 前缀（SetFunctionName 语义），普通方法裸属性名。
        let display_name = match method.kind {
            MethodDefinitionKind::Get => format!("get {method_name}"),
            MethodDefinitionKind::Set => format!("set {method_name}"),
            _ => method_name.to_string(),
        };
        method_module.function_name = Some(display_name);
        method_module.needs_home_object = true;
        ctx.nested.push(method_module);
        let method_reg = ctx.alloc_reg();
        ctx.inst(Inst::create_closure(Operand::Reg(method_reg), ctx.nested.len() as u16));
        ctx.inst(Inst::new(OpCode::SET_HOME_OBJECT, Operand::Reg(method_reg), home_reg, Operand::None));
        Ok(method_reg)
    }
}
