//! class 声明整体 emit：驱动头、原型、方法、字段各阶段，组装最终类对象。
//!
//! 函数：`emit_class`。

use crate::{CompileCtx, Emitter, FunctionBodyContext};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Class, ClassElement, Expression, MethodDefinitionKind, PropertyKey};

impl Emitter {
    pub(crate) fn emit_class(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u32, String> {
        let elements = &class.body.body;
        let mut constructor_method = None;
        let mut instance_field_indices = Vec::new();
        let mut private_names = Vec::<(String, u32)>::new();
        let is_derived = class.super_class.is_some();

        for (idx, element) in elements.iter().enumerate() {
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
                        instance_field_indices.push(idx);
                    }
                }
                ClassElement::AccessorProperty(_) => return Err("class accessor properties not yet supported".into()),
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }

        // 计算键 slot 分配：全部元素源码序（方法 + 公有字段），私有元素非 computed。
        let mut computed_slot = 0u8;
        let key_slots: Vec<Option<u8>> = elements
            .iter()
            .map(|e| {
                let computed = match e {
                    ClassElement::MethodDefinition(m) => m.computed,
                    ClassElement::PropertyDefinition(p) => {
                        p.computed && !matches!(p.key, PropertyKey::PrivateIdentifier(_))
                    }
                    _ => false,
                };
                if computed {
                    let s = computed_slot;
                    computed_slot += 1;
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        let any_computed = computed_slot > 0;

        let (ctor_reg, proto_reg, super_reg) = self.emit_class_header(class, ctx)?;
        let ctor_name = class.id.as_ref().map(|id| id.name.to_string());
        let self_binding = ctor_name.as_deref().map(|name| vec![(name, ctor_reg)]).unwrap_or_default();
        let saved_derived = ctx.in_derived_constructor;
        let saved_private_names = ctx.scopes.private_name_map.clone();
        ctx.in_derived_constructor = is_derived;
        ctx.scopes.private_name_map = private_names.clone();

        // 实例公有字段 computed key 求值于构造器帧外，须类定义期存入数组。构造器以
        // upvalue 捕获该数组：父作用域登记 `@@field_keys`（cell_idx 取现有最大 +1）。
        let has_computed_instance = instance_field_indices.iter().any(|&i| {
            let ClassElement::PropertyDefinition(p) = &elements[i] else { return false };
            p.computed && !matches!(p.key, PropertyKey::PrivateIdentifier(_))
        });
        let field_key_cell: Option<u8> = if has_computed_instance {
            let cell_idx = ctx.captured_bindings.values().copied().max().map_or(0, |m| m.saturating_add(1));
            ctx.captured_bindings.insert("@@field_keys".to_string(), cell_idx);
            Some(cell_idx)
        } else {
            None
        };
        // 字段值表达式运行于构造器帧：其自由变量须纳入构造器 upvalue 捕获。
        let field_value_exprs: Vec<&Expression> = instance_field_indices
            .iter()
            .filter_map(|&i| match &elements[i] {
                ClassElement::PropertyDefinition(p) => p.value.as_ref(),
                _ => None,
            })
            .collect();
        let extra_upvalue_names: Vec<(&str, u8)> = field_key_cell.map(|c| ("@@field_keys", c)).into_iter().collect();

        let emit_instance_fields = |compiler: &Emitter, field_ctx: &mut CompileCtx| -> Result<(), String> {
            for &idx in &instance_field_indices {
                let ClassElement::PropertyDefinition(field) = &elements[idx] else { unreachable!() };
                let field = field.as_ref();
                if let PropertyKey::PrivateIdentifier(private) = &field.key {
                    compiler.emit_private_field_init(
                        Operand::This,
                        private.name.as_str(),
                        field.value.as_ref(),
                        field_ctx,
                    )?;
                } else {
                    compiler.emit_public_field_init(
                        Operand::This,
                        &field.key,
                        field.computed,
                        field.value.as_ref(),
                        key_slots[idx],
                        field_ctx,
                    )?;
                }
            }
            Ok(())
        };

        let mut ctor_module = if let Some(method) = constructor_method {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            self.compile_function_body_with_field_hooks(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(&emit_instance_fields),
                is_derived,
                &field_value_exprs,
                &extra_upvalue_names,
            )?
        } else {
            let mut module = self.compile_function_body_with_field_hooks(
                &[],
                &[],
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(&emit_instance_fields),
                is_derived,
                &field_value_exprs,
                &extra_upvalue_names,
            )?;
            if is_derived {
                module.insts.clear();
                module.constants.clear();
                module.n_registers = 1;
                module.insts.push(Inst::super_call(Operand::None, Operand::None, 0));
                // 字段初始化直接重发：构造器 upvalue/内置槽引用须与首轮编译产物对齐。
                let mut field_ctx = CompileCtx::new();
                field_ctx.scopes.private_name_map = private_names.clone();
                field_ctx.scopes.builtin_reg_map = module.builtin_reg_map.clone();
                field_ctx.current_upvalue_captures = module.upvalue_captures.clone();
                field_ctx.field_keys_uv = module
                    .upvalue_captures
                    .iter()
                    .position(|u| u.name == "@@field_keys")
                    .map(|i| i as u8);
                emit_instance_fields(self, &mut field_ctx)?;
                module.insts.extend(field_ctx.insts);
                module.constants = field_ctx.constants;
                module.n_registers = field_ctx.max_regs.max(1);
                module
                    .insts
                    .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));
            }
            module
        };
        ctx.in_derived_constructor = saved_derived;
        ctor_module.is_class_constructor = true;
        ctor_module.is_derived_constructor = is_derived;
        ctor_module.function_name = ctor_name.clone();
        ctx.nested.push(ctor_module);
        // 键数组构建会在 keys 表达式中压入嵌套模块（箭头/函数/类键），
        // 必须在压入后用固定下标引用构造器模块，避免嵌套长度偏移。
        let ctor_sub_idx = ctx.nested.len() as u16;

        // 类定义期按源码序求值全部 computed key（extends 之后、构造器闭包创建之前），
        // 存数组供方法/静态字段/实例字段初始化阶段按 slot 取用。
        if any_computed {
            let keys_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(
                OpCode::NEW_ARRAY,
                Operand::Reg(keys_reg),
                Operand::Imm(computed_slot as u16),
                Operand::None,
            ));
            for (idx, element) in elements.iter().enumerate() {
                let Some(slot) = key_slots[idx] else { continue };
                let key_expr = match element {
                    ClassElement::MethodDefinition(m) => m.key.as_expression(),
                    ClassElement::PropertyDefinition(p) => p.key.as_expression(),
                    _ => None,
                };
                let Some(expr) = key_expr else { continue };
                let key_reg = self.emit_expression(expr, ctx)?;
                let idx_reg = ctx.alloc_reg();
                let idx_const = ctx.add_constant(Constant::Int(slot as i32));
                ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx_const));
                ctx.inst(Inst::new(
                    OpCode::SET_ELEM,
                    Operand::Reg(keys_reg),
                    Operand::Reg(idx_reg),
                    Operand::Reg(key_reg),
                ));
            }
            if let Some(cell_idx) = field_key_cell {
                ctx.inst(Inst::new(
                    OpCode::MAKE_CELL,
                    Operand::Reg(keys_reg),
                    Operand::Imm(cell_idx as u16),
                    Operand::None,
                ));
            }
            ctx.class_keys_reg = Some(keys_reg);
        }

        self.emit_class_prototype(ctor_reg, proto_reg, super_reg, ctor_sub_idx, ctx)?;
        self.emit_class_methods(&class.body.body, ctor_reg, proto_reg, &self_binding, &key_slots, ctx)?;
        self.emit_class_static_elements(&class.body.body, ctor_reg, &key_slots, ctx)?;

        ctx.scopes.private_name_map = saved_private_names;
        Ok(ctor_reg)
    }
}
