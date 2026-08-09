//! 解构绑定/赋值 emit：数组与对象 pattern 的绑定初始化、rest 收集与解构赋值。
//! 函数：`emit_bind_target`、`emit_binding_pattern`、`emit_array_binding`、
//! `emit_object_binding`、`emit_assign_target` 等。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{
    ArrayAssignmentTarget, ArrayPattern, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty,
    BindingPattern, Expression, ObjectAssignmentTarget, ObjectPattern, VariableDeclarationKind,
};

impl Emitter {
    pub(crate) fn emit_default_if_undefined(
        &self, val_reg: u32, default_expr: &Expression, name: Option<&str>, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let undef_reg = self.emit_undefined(ctx);
        let eq_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::STRICT_EQ,
            Operand::Reg(eq_reg),
            Operand::Reg(val_reg),
            Operand::Reg(undef_reg),
        ));
        // val != undefined（eq 为 false）→ 跳过默认值
        let end_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(eq_reg, end_label));
        let default_reg = self.emit_expression(default_expr, ctx)?;
        // 默认值表达式是匿名函数/箭头/class 时按绑定名推断 name（SetFunctionName）。
        if let Some(name) = name {
            if crate::is_anonymous_function_definition(default_expr) {
                if let Some(sub_mod) = ctx.nested.last_mut() {
                    if sub_mod.function_name.is_none() {
                        sub_mod.function_name = Some(name.to_string());
                    }
                }
            }
        }
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(val_reg),
            Operand::Reg(default_reg),
            Operand::None,
        ));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        Ok(val_reg)
    }

    fn emit_bind_target(
        &self, name: &str, src_reg: u32, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let target_reg = if matches!(kind, VariableDeclarationKind::Var) {
            // `var` 名已预声明（hoisting），复用预登记槽位而非新分配，使 n_registers 与 let/const 一致。
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                reg
            } else {
                let var_reg = ctx.alloc_reg();
                ctx.declare(name, var_reg, kind, is_const)?;
                var_reg
            }
        } else {
            let var_reg = ctx.alloc_reg();
            ctx.declare(name, var_reg, kind, is_const)?;
            var_reg
        };
        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL,
                Operand::Reg(src_reg),
                Operand::Imm(cell_idx as u16),
                Operand::None,
            ));
        } else if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
            // with 内 var 初始化：对象有该属性则写对象，否则写提升槽（动态解析）。
            self.emit_with_dynamic_write(name, src_reg, 0, ctx);
        } else {
            // const 声明路径 STORE_VAR 恒 b=0，不查运行时 guard。
            // guard 读 regs[rd] 判断槽是否已初始化，依赖槽初始为 undefined；
            // 声明冗余检查由编译期 declare() 完成（重复声明编译报错），
            // 运行时 guard 只服务"对 const 再赋值"（赋值路径，见 emit_assign_target）。
            ctx.inst(Inst::new(
                OpCode::STORE_VAR,
                Operand::Reg(target_reg),
                Operand::Reg(src_reg),
                Operand::Imm(0),
            ));
        }
        ctx.init_var(name);
        Ok(())
    }

    pub(crate) fn emit_assign_target(
        &self, target: &AssignmentTarget, src_reg: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(var_reg),
                    Operand::Reg(src_reg),
                    Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
                ));
                Ok(())
            }
            AssignmentTarget::ArrayAssignmentTarget(ap) => self.emit_array_assignment(ap, src_reg, ctx),
            AssignmentTarget::ObjectAssignmentTarget(op) => self.emit_object_assignment(op, src_reg, ctx),
            _ => Err("assignment target not supported".into()),
        }
    }

    pub(crate) fn emit_binding_pattern(
        &self, pattern: &BindingPattern, src_reg: u32, kind: VariableDeclarationKind, is_const: bool,
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => {
                self.emit_bind_target(bi.name.as_str(), src_reg, kind, is_const, ctx)
            }
            BindingPattern::ArrayPattern(ap) => self.emit_array_binding(ap, src_reg, kind, is_const, ctx),
            BindingPattern::ObjectPattern(op) => self.emit_object_binding(op, src_reg, kind, is_const, ctx),
            BindingPattern::AssignmentPattern(ap) => {
                // `[x = fn]`：默认值函数按绑定名推断 name。
                let name = match &ap.left {
                    BindingPattern::BindingIdentifier(bi) => Some(bi.name.as_str()),
                    _ => None,
                };
                let val_reg = self.emit_default_if_undefined(src_reg, &ap.right, name, ctx)?;
                self.emit_binding_pattern(&ap.left, val_reg, kind, is_const, ctx)
            }
        }
    }

    fn emit_array_binding(
        &self, ap: &ArrayPattern, src_reg: u32, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(src_reg), Operand::None));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg), Operand::None, Operand::None));
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
            if let Some(pattern) = elem {
                self.emit_binding_pattern(pattern, val_reg, kind, is_const, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_binding_pattern(&rest.argument, rest_reg, kind, is_const, ctx)?;
        }
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        Ok(())
    }

    fn emit_collect_rest_array(&self, ctx: &mut CompileCtx) -> Result<u32, String> {
        let rest_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(rest_reg), Operand::None, Operand::None));
        let idx_reg = ctx.alloc_reg();
        let zero_idx = ctx.add_constant(Constant::Int(0));
        ctx.inst(Inst::load_const(Operand::Reg(idx_reg), zero_idx));
        let loop_start = ctx.next_label_id();
        let loop_end = ctx.next_label_id();
        ctx.labels.set_label_pos(loop_start, ctx.insts.len());
        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg), Operand::None, Operand::None));
        ctx.inst(Inst::jmp_if_false(has_reg, loop_end));
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
        ctx.inst(Inst::new(
            OpCode::SET_ELEM,
            Operand::Reg(rest_reg),
            Operand::Reg(idx_reg),
            Operand::Reg(val_reg),
        ));
        let tmp_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::INC_PRE,
            Operand::Reg(idx_reg),
            Operand::Reg(tmp_reg),
            Operand::Reg(tmp_reg),
        ));
        ctx.inst(Inst::jmp(loop_start));
        ctx.labels.set_label_pos(loop_end, ctx.insts.len());
        Ok(rest_reg)
    }

    fn emit_object_binding(
        &self, op: &ObjectPattern, src_reg: u32, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        // 对象解构先 ToObject(rhs)：null/undefined 抛 TypeError（含空 pattern），
        // 原始值包装为对应对象，保证属性读取与 rest 都以对象为源。
        // 写到临时寄存器，保留 src_reg 原值（声明初始化路径的返回值语义）。
        let obj_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(obj_reg), Operand::Reg(src_reg), Operand::None));
        ctx.inst(Inst::new(OpCode::TO_OBJECT, Operand::Reg(obj_reg), Operand::None, Operand::None));
        // 有 rest 时构建运行时 excluded 数组：静态 key 直接 push 常量，computed key 求值结果 push 值。
        let mut excluded = Vec::new();
        let mut excl_arr_reg = None;
        let mut excl_push = 0usize;
        if op.rest.is_some() {
            excl_arr_reg = Some(self.emit_new_excluded_array(ctx));
        }
        for prop in &op.properties {
            let (prop_reg, static_key, key_reg) =
                self.emit_object_property_read_key(obj_reg, &prop.key, prop.computed, ctx)?;
            if let Some(key) = static_key {
                excluded.push(key.clone());
                if let Some(arr) = excl_arr_reg {
                    self.emit_push_excluded_key(arr, None, &key, excl_push, ctx);
                    excl_push += 1;
                }
            } else if let (Some(arr), Some(key_reg)) = (excl_arr_reg, key_reg) {
                self.emit_push_excluded_key(arr, Some(key_reg), "", excl_push, ctx);
                excl_push += 1;
            }
            self.emit_binding_pattern(&prop.value, prop_reg, kind, is_const, ctx)?;
        }
        if let Some(rest) = &op.rest {
            let rest_reg = ctx.alloc_reg();
            let excluded_idx = ctx.add_constant(Constant::String(excluded.join("\0")));
            ctx.inst(Inst::rest_object(
                Operand::Reg(rest_reg),
                Operand::Reg(obj_reg),
                excluded_idx as u32,
                excl_arr_reg.map(Operand::Reg),
            ));
            self.emit_binding_pattern(&rest.argument, rest_reg, kind, is_const, ctx)?;
        }
        Ok(())
    }

    /// 新建运行时 excluded 数组（NEW_ARRAY），供 REST_OBJECT 排除 computed key。
    fn emit_new_excluded_array(&self, ctx: &mut CompileCtx) -> u32 {
        let arr = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(arr), Operand::None, Operand::None));
        arr
    }

    /// 向 excluded 数组下标 `idx` 写入一个键：`key_val` 为运行时寄存器值，否则用 `key_str` 常量。
    fn emit_push_excluded_key(
        &self, arr: u32, key_val: Option<u32>, key_str: &str, idx: usize, ctx: &mut CompileCtx,
    ) {
        let val_reg = match key_val {
            Some(kv) => kv,
            None => {
                let ki = ctx.add_constant(Constant::String(key_str.to_string()));
                let kr = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(kr), ki));
                kr
            }
        };
        let elem_idx = ctx.alloc_reg();
        let cidx = ctx.add_constant(Constant::Int(idx as i32));
        ctx.inst(Inst::load_const(Operand::Reg(elem_idx), cidx));
        ctx.inst(Inst::new(OpCode::SET_ELEM, Operand::Reg(arr), Operand::Reg(elem_idx), Operand::Reg(val_reg)));
    }

    fn emit_assignment_maybe_default(
        &self, target: &AssignmentTargetMaybeDefault, src_reg: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(default) => {
                let name = match &default.binding {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => Some(id.name.as_str()),
                    _ => None,
                };
                let val_reg = self.emit_default_if_undefined(src_reg, &default.init, name, ctx)?;
                self.emit_assign_target(&default.binding, val_reg, ctx)
            }
            AssignmentTargetMaybeDefault::ArrayAssignmentTarget(ap) => self.emit_array_assignment(ap, src_reg, ctx),
            AssignmentTargetMaybeDefault::ObjectAssignmentTarget(op) => self.emit_object_assignment(op, src_reg, ctx),
            AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(var_reg),
                    Operand::Reg(src_reg),
                    Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
                ));
                Ok(())
            }
            _ => Err("assignment target not supported".into()),
        }
    }

    pub(crate) fn emit_array_assignment(
        &self, ap: &ArrayAssignmentTarget, src_reg: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(src_reg), Operand::None));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg), Operand::None, Operand::None));
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
            if let Some(target) = elem {
                self.emit_assignment_maybe_default(target, val_reg, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        Ok(())
    }

    pub(crate) fn emit_object_assignment(
        &self, op: &ObjectAssignmentTarget, src_reg: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        // 对象解构先 ToObject(rhs)：null/undefined 抛 TypeError（含空 pattern），
        // 原始值包装为对应对象，保证属性读取与 rest 都以对象为源。
        // 写到临时寄存器，保留 src_reg 原值（解构赋值表达式返回值 = rhs）。
        let obj_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(obj_reg), Operand::Reg(src_reg), Operand::None));
        ctx.inst(Inst::new(OpCode::TO_OBJECT, Operand::Reg(obj_reg), Operand::None, Operand::None));
        let mut excluded = Vec::new();
        let mut excl_arr_reg = None;
        let mut excl_push = 0usize;
        if op.rest.is_some() {
            excl_arr_reg = Some(self.emit_new_excluded_array(ctx));
        }
        for prop in &op.properties {
            match prop {
                AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    let key = id.binding.name.as_str().to_string();
                    excluded.push(key.clone());
                    if let Some(arr) = excl_arr_reg {
                        self.emit_push_excluded_key(arr, None, &key, excl_push, ctx);
                        excl_push += 1;
                    }
                    let mut prop_reg = self.emit_object_property_read(obj_reg, &key, ctx);
                    if let Some(default_expr) = &id.init {
                        let name = id.binding.name.as_str();
                        prop_reg = self.emit_default_if_undefined(prop_reg, default_expr, Some(name), ctx)?;
                    }
                    let name = id.binding.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    ctx.inst(Inst::new(
                        OpCode::STORE_VAR,
                        Operand::Reg(var_reg),
                        Operand::Reg(prop_reg),
                        Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
                    ));
                }
                AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                    let (prop_reg, static_key, key_reg) =
                        self.emit_object_property_read_key(obj_reg, &prop.name, prop.computed, ctx)?;
                    if let Some(key) = static_key {
                        excluded.push(key.clone());
                        if let Some(arr) = excl_arr_reg {
                            self.emit_push_excluded_key(arr, None, &key, excl_push, ctx);
                            excl_push += 1;
                        }
                    } else if let (Some(arr), Some(key_reg)) = (excl_arr_reg, key_reg) {
                        self.emit_push_excluded_key(arr, Some(key_reg), "", excl_push, ctx);
                        excl_push += 1;
                    }
                    self.emit_assignment_maybe_default(&prop.binding, prop_reg, ctx)?;
                }
            }
        }
        if let Some(rest) = &op.rest {
            let rest_reg = ctx.alloc_reg();
            let excluded_idx = ctx.add_constant(Constant::String(excluded.join("\0")));
            ctx.inst(Inst::rest_object(
                Operand::Reg(rest_reg),
                Operand::Reg(obj_reg),
                excluded_idx as u32,
                excl_arr_reg.map(Operand::Reg),
            ));
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        Ok(())
    }
}
