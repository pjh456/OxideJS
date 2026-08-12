//! class 声明整体 emit：驱动头、原型、方法、字段各阶段，组装最终类对象。
//!
//! 函数：`emit_class`。

use crate::{symbol_table::ScopeKind, CompileCtx, Emitter, FunctionBodyContext};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Class, ClassElement, Expression, MethodDefinitionKind, PropertyKey, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_class(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 类表达式：类名在类体内为 const 绑定；不向调用方复用寄存器时，
        // 由 emit_class_with_binding 新建块作用域声明（TDZ），类构建完成后
        // STORE_VAR 初始化再弹出作用域，类名不泄漏到外层。
        self.emit_class_with_binding(class, ctx, None)
    }

    /// 类名绑定可复用调用方寄存器的类整体 emit。
    ///
    /// 类声明由调用方先在外层声明绑定（未初始化），`binding_reg` 复用该槽；
    /// 类表达式无外部槽时，在独立块作用域声明 const 绑定（未初始化）。
    /// 无论哪种形态，绑定都在 `extends` 求值前建立：extends 引用类名按规范
    /// 抛 TDZ ReferenceError；类构建完成后 `init_var` + `STORE_VAR` 初始化。
    pub(crate) fn emit_class_with_binding(
        &self, class: &Class, ctx: &mut CompileCtx, binding_reg: Option<u32>,
    ) -> Result<u32, String> {
        let ctor_name = class.id.as_ref().map(|id| id.name.to_string());
        let mut pushed_scope = false;
        let binding_reg = if let Some(name) = ctor_name.as_deref() {
            match binding_reg {
                Some(reg) => {
                    pushed_scope = false;
                    reg
                }
                None => {
                    let reg = ctx.alloc_reg();
                    ctx.push_scope_with_kind(ScopeKind::BlockScope);
                    ctx.declare(name, reg, VariableDeclarationKind::Const, true)?;
                    pushed_scope = true;
                    reg
                }
            }
        } else {
            0
        };
        let elements = &class.body.body;
        let mut constructor_method = None;
        let mut instance_field_indices = Vec::new();
        let mut private_names = Vec::<(String, u32, Option<MethodDefinitionKind>, bool)>::new();
        let is_derived = class.super_class.is_some();

        for (idx, element) in elements.iter().enumerate() {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if let PropertyKey::PrivateIdentifier(private) = &method.key {
                        let name = private.name.as_str().to_string();
                        let kind = method.kind;
                        match private_names.iter().find(|(n, _, _, _)| n == &name) {
                            // 同名 getter/setter 构成访问器对，复用同一私有名。
                            Some((_, _, Some(MethodDefinitionKind::Get), _)) if kind == MethodDefinitionKind::Set => {}
                            Some((_, _, Some(MethodDefinitionKind::Set), _)) if kind == MethodDefinitionKind::Get => {}
                            Some(_) => return Err(format!("duplicate private name #{name}")),
                            None => {
                                let id = ctx.scopes.next_private_name_id;
                                ctx.scopes.next_private_name_id = ctx.scopes.next_private_name_id.saturating_add(1);
                                private_names.push((name, id, Some(kind), method.r#static));
                            }
                        }
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
                        if private_names.iter().any(|(existing, _, _, _)| existing == &name) {
                            return Err(format!("duplicate private name #{name}"));
                        }
                        let id = ctx.scopes.next_private_name_id;
                        ctx.scopes.next_private_name_id = ctx.scopes.next_private_name_id.saturating_add(1);
                        private_names.push((name, id, None, prop.r#static));
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
        let self_binding = ctor_name.as_deref().map(|name| vec![(name, binding_reg)]).unwrap_or_default();
        // Class-name binding cell: class elements (ctor/method/field) that reference the
        // class name capture it through this dedicated cell. Reads inside the class then
        // resolve via LOAD_UPVALUE (same cell as the binding), so identity with other
        // references survives epoch promotion (which rewrites cell values) and inline
        // accessor calls (which clear the register file). The synthetic key keeps user
        // bindings with the same name (outer scopes) untouched.
        let class_self_cell: Option<u8> = ctor_name.as_deref().map(|_| {
            let cell_idx = ctx
                .captured_bindings
                .values()
                .copied()
                .max()
                .map_or(0, |m| m.saturating_add(1));
            ctx.captured_bindings.insert(format!("@@class_self_{cell_idx}"), cell_idx);
            cell_idx
        });

        let saved_derived = ctx.in_derived_constructor;
        let saved_private_names = ctx.scopes.private_name_map.clone();
        let saved_private_kinds = ctx.scopes.private_element_kinds.clone();
        let saved_brand_id = ctx.scopes.private_brand_id;
        ctx.in_derived_constructor = is_derived;
        ctx.scopes.private_name_map = private_names.iter().map(|(n, id, _, _)| (n.clone(), *id)).collect();
        ctx.scopes.private_element_kinds = private_names
            .iter()
            .map(|(n, _, kind, is_static)| (n.clone(), *kind, *is_static))
            .collect();

        // 类 brand：有私有元素时分配 brand 私有名 id，并创建 brand 对象（= 类原型）。
        // 构造器把 brand 槽写入实例 own；私有方法/访问器/静态字段访问（GET/SET）据
        // brand 对象同一性做检查；instance 字段走 PrivateFieldFind 原型链查找，不检查。
        let private_brand_id = if !private_names.is_empty() {
            let id = ctx.scopes.next_private_name_id;
            ctx.scopes.next_private_name_id = ctx.scopes.next_private_name_id.saturating_add(1);
            Some(id)
        } else {
            None
        };
        ctx.scopes.private_brand_id = private_brand_id;

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
        // 类 brand 对象（= proto）由类定义期 MAKE_CELL 写入，构造器经 upvalue 捕获。
        let brand_cell: Option<u8> = private_brand_id.map(|_| {
            let cell_idx = ctx.captured_bindings.values().copied().max().map_or(0, |m| m.saturating_add(1));
            ctx.captured_bindings.insert("@@class_brand".to_string(), cell_idx);
            cell_idx
        });
        // 字段值表达式运行于构造器帧：其自由变量须纳入构造器 upvalue 捕获。
        let field_value_exprs: Vec<&Expression> = instance_field_indices
            .iter()
            .filter_map(|&i| match &elements[i] {
                ClassElement::PropertyDefinition(p) => p.value.as_ref(),
                _ => None,
            })
            .collect();
        let mut extra_upvalue_names: Vec<(&str, u8)> =
            field_key_cell.map(|c| ("@@field_keys", c)).into_iter().collect();
        if let Some(c) = brand_cell {
            extra_upvalue_names.push(("@@class_brand", c));
        }
        if let (Some(name), Some(c)) = (ctor_name.as_deref(), class_self_cell) {
            extra_upvalue_names.push((name, c));
        }

        let emit_instance_fields = |compiler: &Emitter, field_ctx: &mut CompileCtx| -> Result<(), String> {
            // 先写 brand 槽（私有方法/访问器的 brand 检查依据），再初始化字段。
            if let Some(bid) = private_brand_id {
                if brand_cell.is_none() {
                    return Err("class brand cell missing".into());
                }
                let brand_reg = field_ctx.alloc_reg();
                let uv_idx = field_ctx
                    .current_upvalue_captures
                    .iter()
                    .position(|u| u.name == "@@class_brand")
                    .ok_or("class brand upvalue missing")? as u16;
                field_ctx.inst(Inst::new(
                    OpCode::LOAD_UPVALUE,
                    Operand::Reg(brand_reg),
                    Operand::Imm(uv_idx),
                    Operand::None,
                ));
                let key_idx = field_ctx.add_constant(Constant::Int(bid as i32));
                let brand_key_reg = field_ctx.alloc_reg();
                field_ctx.inst(Inst::load_const(Operand::Reg(brand_key_reg), key_idx));
                field_ctx.inst(Inst::init_private(
                    Operand::This,
                    Operand::Reg(brand_reg),
                    Operand::Reg(brand_key_reg),
                    false,
                ));
            }
            for &idx in &instance_field_indices {
                let ClassElement::PropertyDefinition(field) = &elements[idx] else {
                    unreachable!()
                };
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
                module.n_registers = 2;
                // 隐式派生构造器等价于 `constructor(...args) { super(...args); }`：
                // 收集全部实参为数组，再展开传给 super。
                module.insts.push(Inst::create_rest_array(Operand::Reg(1), 0));
                module.insts.push(Inst::super_call_spread(Operand::Reg(0), &[0x8000_0000 | 1]));
                // 字段初始化直接重发：构造器 upvalue/内置槽引用须与首轮编译产物对齐。
                let mut field_ctx = CompileCtx::new();
                field_ctx.scopes.private_name_map =
                    private_names.iter().map(|(n, id, _, _)| (n.clone(), *id)).collect();
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
        // brand 对象即类原型：构造器对象（静态私有访问）与实例（实例私有访问）各写
        // brand 槽；构造器经 @@class_brand upvalue 捕获原型对象。
        if let (Some(bid), Some(cell_idx)) = (private_brand_id, brand_cell) {
            let key_idx = ctx.add_constant(Constant::Int(bid as i32));
            let brand_key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(brand_key_reg), key_idx));
            ctx.inst(Inst::init_private(
                Operand::Reg(ctor_reg),
                Operand::Reg(proto_reg),
                Operand::Reg(brand_key_reg),
                false,
            ));
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL,
                Operand::Reg(proto_reg),
                Operand::Imm(cell_idx as u16),
                Operand::None,
            ));
        }
        self.emit_class_methods(&class.body.body, ctor_reg, proto_reg, &self_binding, class_self_cell, &key_slots, ctx)?;
        self.emit_class_static_elements(&class.body.body, ctor_reg, &key_slots, ctx)?;


        // 类构建完成：初始化类名绑定并写入类构造器（类内方法引用该寄存器/捕获）。
        if let Some(name) = ctor_name.as_deref() {
            ctx.init_var(name);
            ctx.inst(Inst::new(
                OpCode::STORE_VAR,
                Operand::Reg(binding_reg),
                Operand::Reg(ctor_reg),
                Operand::None,
            ));
            if let Some(cell_idx) = class_self_cell {
                // Initialize the class-name cell with the constructor and release TDZ
                // (the placeholder cell was created by the element closures at hoist time).
                ctx.inst(Inst::new(
                    OpCode::MAKE_CELL,
                    Operand::Reg(binding_reg),
                    Operand::Imm(cell_idx as u16),
                    Operand::None,
                ));
            }
        }

        if pushed_scope {
            ctx.pop_scope();
        }
        ctx.scopes.private_name_map = saved_private_names;
        ctx.scopes.private_element_kinds = saved_private_kinds;
        ctx.scopes.private_brand_id = saved_brand_id;
        Ok(ctor_reg)
    }
}
