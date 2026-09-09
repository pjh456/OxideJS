//! 赋值表达式 emit：简单 `=`、复合赋值与解构赋值（数组/对象 pattern）。
//! 函数：`emit_assignment_expression`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::AssignmentOperator;

impl Emitter {
    /// 标识符复合赋值的静态路径：upvalue / 被捕获 cell 走显式读取-运算-写回
    /// （写穿透共享单元），普通槽用 COMPOUND_* 寄存器内 RMW。返回结果寄存器。
    ///
    /// # 边界与前提
    /// - `op` 必须是 COMPOUND_* 族（rd 兼作 lhs 源，a 为 rhs）。
    /// - upvalue / cell 场景忽略 const 语义（共享单元由闭包机制保证）。
    /// - 不可写全局内置槽：值照算（表达式值 = 运算结果）但跳过槽写；strict 抛
    ///   TypeError。局部遮蔽绑定不受影响。
    fn emit_compound_identifier_static(
        &self, name: &str, op: OpCode, rhs: u32, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // const 复合赋值恒抛 TypeError（编译期拦截，值无关）；检查在读旧值之前。
        // 循环 update 段的 let/const 循环变量是 per-iteration 可变绑定，复合写同样合法，豁免。
        if !ctx.register_update_names.iter().any(|n| n == name) {
            self.emit_const_write_guard(name, ctx)?;
        }
        // 循环 update 段：被捕获绑定走寄存器 RMW（C 风格 for 每迭代 fresh，
        // update 写寄存器供下一迭代 fresh 拷贝，不污染本迭代闭包捕获的 cell）。
        if ctx.register_update_names.iter().any(|n| n == name) {
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                if ctx.targets_readonly_builtin(name, reg) {
                    if ctx.is_strict {
                        return self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
                    }
                    // 全局不可写内置：值照算（表达式值 = 运算结果），跳过槽写。
                    let val_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(val_reg), Operand::Reg(reg), Operand::None));
                    ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                    return Ok(val_reg);
                }
                ctx.inst(Inst::new(op, Operand::Reg(reg), Operand::Reg(rhs), Operand::None));
                return Ok(reg);
            }
        }
        let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
        let captured_cell = ctx.captured_bindings.get(name).copied();
        let const_flag = if ctx.lookup_const_flag(name) { 1 } else { 0 };
        if let Some(uv) = uv_idx {
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(
                OpCode::LOAD_UPVALUE,
                Operand::Reg(val_reg),
                Operand::Imm(uv as u16),
                Operand::None,
            ));
            ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
            ctx.inst(Inst::new(
                OpCode::STORE_UPVALUE,
                Operand::Imm(const_flag),
                Operand::Reg(val_reg),
                Operand::Imm(uv as u16),
            ));
            Ok(val_reg)
        } else if let Some(cell_idx) = captured_cell {
            let val_reg = ctx.alloc_reg();
            let a_operand = match ctx.scopes.symbols.lookup_any_binding(name) {
                Some((binding, _)) => Operand::Reg(binding.reg),
                None => Operand::None,
            };
            ctx.inst(Inst::new(
                OpCode::CELL_GET,
                Operand::Reg(val_reg),
                a_operand,
                Operand::Imm(cell_idx as u16),
            ));
            ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
            ctx.inst(Inst::new(
                OpCode::CELL_SET,
                Operand::None,
                Operand::Reg(val_reg),
                Operand::Imm(cell_idx as u16),
            ));
            Ok(val_reg)
        } else {
            let var_reg = ctx.lookup_or_global(name);
            if ctx.targets_readonly_builtin(name, var_reg) {
                // 全局不可写内置槽：strict 抛 TypeError（put 失败）；sloppy 值照算
                // （表达式值 = 运算结果），跳过槽写。
                if ctx.is_strict {
                    return self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
                }
                let val_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(val_reg), Operand::Reg(var_reg), Operand::None));
                ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                return Ok(val_reg);
            }
            let is_implicit = ctx.implicit_global_writes.contains(&var_reg);
            if is_implicit && ctx.is_strict {
                // 严格模式未声明复合写：未解析引用不可 put，值无关抛 ReferenceError。
                return self.emit_strict_undeclared_write(name, ctx);
            }
            ctx.inst(Inst::new(op, Operand::Reg(var_reg), Operand::Reg(rhs), Operand::None));
            if is_implicit {
                self.emit_implicit_global_write(name, var_reg, ctx);
            }
            Ok(var_reg)
        }
    }

    pub(crate) fn emit_assignment_expression(
        &self, assign: &oxide_parser::AssignmentExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(obj_reg),
                    Operand::None,
                ));
                ctx.inst(Inst::ic_get(Operand::Reg(result_reg), Operand::Reg(key_reg)));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(val_reg),
                    Operand::None,
                ));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            let prop_name = member.property.name.as_str();
            let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            if assign.operator != AssignmentOperator::Assign {
                let obj = Operand::Reg(obj_reg);
                let val = Operand::Reg(val_reg);
                let key = Operand::Reg(key_reg);
                match assign.operator {
                    AssignmentOperator::Addition => ctx.inst(Inst::compound_member_add(obj, val, key)),
                    AssignmentOperator::Subtraction => ctx.inst(Inst::compound_member_sub(obj, val, key)),
                    AssignmentOperator::Multiplication => ctx.inst(Inst::compound_member_mul(obj, val, key)),
                    AssignmentOperator::Division => ctx.inst(Inst::compound_member_div(obj, val, key)),
                    AssignmentOperator::Remainder => ctx.inst(Inst::compound_member_mod(obj, val, key)),
                    AssignmentOperator::Exponential => ctx.inst(Inst::compound_member_exp(obj, val, key)),
                    AssignmentOperator::BitwiseAnd => ctx.inst(Inst::compound_member_bit_and(obj, val, key)),
                    AssignmentOperator::BitwiseOR => ctx.inst(Inst::compound_member_bit_or(obj, val, key)),
                    AssignmentOperator::BitwiseXOR => ctx.inst(Inst::compound_member_bit_xor(obj, val, key)),
                    AssignmentOperator::ShiftLeft => ctx.inst(Inst::compound_member_shl(obj, val, key)),
                    AssignmentOperator::ShiftRight => ctx.inst(Inst::compound_member_shr(obj, val, key)),
                    AssignmentOperator::ShiftRightZeroFill => ctx.inst(Inst::compound_member_ushr(obj, val, key)),
                    _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                }
                Ok(val_reg)
            } else {
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                Ok(val_reg)
            }
        } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
            // 常量字符串键折叠为 IC 静态路径，与 StaticMemberExpression 分支同构。
            if let Some(key) = crate::expr::member::computed_const_key(&member.expression) {
                if let Some(logical_op) = assign.operator.to_logical_operator() {
                    let store_label = ctx.next_label_id();
                    let end_label = ctx.next_label_id();
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let idx = ctx.add_constant(Constant::String(key));
                    let key_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(obj_reg),
                        Operand::None,
                    ));
                    ctx.inst(Inst::ic_get(Operand::Reg(result_reg), Operand::Reg(key_reg)));
                    self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                    ctx.labels.set_label_pos(store_label, ctx.insts.len());
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(val_reg),
                        Operand::None,
                    ));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
                    return Ok(result_reg);
                }
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let idx = ctx.add_constant(Constant::String(key));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                if assign.operator != AssignmentOperator::Assign {
                    // 复合赋值保规范求值序：先读属性（ic_get）再求值 RHS——RHS 副作用
                    // 可能改写同一属性，后求值才能读到旧值。
                    let val_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(val_reg), Operand::Reg(obj_reg), Operand::None));
                    ctx.inst(Inst::ic_get(Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    let rhs = self.emit_expression(&assign.right, ctx)?;
                    let op = match assign.operator {
                        AssignmentOperator::Addition => OpCode::ADD,
                        AssignmentOperator::Subtraction => OpCode::SUB,
                        AssignmentOperator::Multiplication => OpCode::MUL,
                        AssignmentOperator::Division => OpCode::DIV,
                        AssignmentOperator::Remainder => OpCode::MOD,
                        AssignmentOperator::Exponential => OpCode::COMPOUND_EXP,
                        AssignmentOperator::BitwiseAnd => OpCode::BIT_AND,
                        AssignmentOperator::BitwiseOR => OpCode::BIT_OR,
                        AssignmentOperator::BitwiseXOR => OpCode::BIT_XOR,
                        AssignmentOperator::ShiftLeft => OpCode::SHL,
                        AssignmentOperator::ShiftRight => OpCode::SHR,
                        AssignmentOperator::ShiftRightZeroFill => OpCode::USHR,
                        _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                    };
                    if assign.operator == AssignmentOperator::Exponential {
                        // 指数无独立二元指令，COMPOUND_EXP 语义 rd=rd^a：rhs 放 a 槽。
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                    } else {
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(rhs)));
                    }
                    ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    Ok(val_reg)
                } else {
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    Ok(val_reg)
                }
            } else {
                self.emit_computed_member_dynamic(assign, member, ctx)
            }
        } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
            let name = member.field.name.as_str();
            // 逻辑赋值：GET_PRIVATE 读旧值（brand 检查在读时执行）→ 短路测试 →
            // 通过才求值 RHS 并 SET_PRIVATE 写回，结果统一为旧值或新值。
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
                let key_reg = self.emit_private_id_reg(name, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::get_private(
                    Operand::Reg(result_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::set_private(
                    Operand::Reg(obj_reg),
                    Operand::Reg(val_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(val_reg),
                    Operand::None,
                ));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
            let key_reg = self.emit_private_id_reg(name, ctx)?;
            if assign.operator != AssignmentOperator::Assign {
                // 复合赋值保规范求值序：先 GET_PRIVATE 读旧值（brand 检查先于 RHS 副作用），
                // 再求值 RHS，运算后 SET_PRIVATE 写回。
                let val_reg = ctx.alloc_reg();
                ctx.inst(Inst::get_private(
                    Operand::Reg(val_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                let rhs = self.emit_expression(&assign.right, ctx)?;
                let op = match assign.operator {
                    AssignmentOperator::Addition => OpCode::ADD,
                    AssignmentOperator::Subtraction => OpCode::SUB,
                    AssignmentOperator::Multiplication => OpCode::MUL,
                    AssignmentOperator::Division => OpCode::DIV,
                    AssignmentOperator::Remainder => OpCode::MOD,
                    AssignmentOperator::Exponential => OpCode::COMPOUND_EXP,
                    AssignmentOperator::BitwiseAnd => OpCode::BIT_AND,
                    AssignmentOperator::BitwiseOR => OpCode::BIT_OR,
                    AssignmentOperator::BitwiseXOR => OpCode::BIT_XOR,
                    AssignmentOperator::ShiftLeft => OpCode::SHL,
                    AssignmentOperator::ShiftRight => OpCode::SHR,
                    AssignmentOperator::ShiftRightZeroFill => OpCode::USHR,
                    _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                };
                if assign.operator == AssignmentOperator::Exponential {
                    // 指数无独立二元指令，COMPOUND_EXP 语义 rd=rd^a：rhs 放 a 槽。
                    ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                } else {
                    ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(rhs)));
                }
                ctx.inst(Inst::set_private(
                    Operand::Reg(obj_reg),
                    Operand::Reg(val_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                Ok(val_reg)
            } else {
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::set_private(
                    Operand::Reg(obj_reg),
                    Operand::Reg(val_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                Ok(val_reg)
            }
        } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) = &assign.left {
            if assign.operator != AssignmentOperator::Assign {
                if let Some(logical_op) = assign.operator.to_logical_operator() {
                    let store_label = ctx.next_label_id();
                    let end_label = ctx.next_label_id();
                    let name = id_ref.name.as_str();
                    // 逻辑赋值先解析赋值引用：TDZ 绑定在读旧值前抛 ReferenceError。
                    self.emit_identifier_tdz_guard(name, ctx)?;
                    // 目标判定：upvalue / 被捕获 cell / 普通槽，读与写须穿透共享单元。
                    let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
                    let captured_cell = ctx.captured_bindings.get(name).copied();
                    let result_reg = ctx.alloc_reg();
                    if let Some(uv) = uv_idx {
                        ctx.inst(Inst::new(
                            OpCode::LOAD_UPVALUE,
                            Operand::Reg(result_reg),
                            Operand::Imm(uv as u16),
                            Operand::None,
                        ));
                    } else if let Some(cell_idx) = captured_cell {
                        let a_operand = match ctx.scopes.symbols.lookup_any_binding(name) {
                            Some((binding, _)) => Operand::Reg(binding.reg),
                            None => Operand::None,
                        };
                        ctx.inst(Inst::new(
                            OpCode::CELL_GET,
                            Operand::Reg(result_reg),
                            a_operand,
                            Operand::Imm(cell_idx as u16),
                        ));
                    } else {
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.inst(Inst::new(
                            OpCode::LOAD_VAR,
                            Operand::Reg(result_reg),
                            Operand::Reg(var_reg),
                            Operand::None,
                        ));
                    }
                    self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                    ctx.labels.set_label_pos(store_label, ctx.insts.len());
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    // 短路未通过才写：const 目标在此抛 TypeError（编译期拦截，值无关）。
                    self.emit_const_write_guard(name, ctx)?;
                    let const_flag = if ctx.lookup_const_flag(name) { 1 } else { 0 };
                    if let Some(uv) = uv_idx {
                        ctx.inst(Inst::new(
                            OpCode::STORE_UPVALUE,
                            Operand::Imm(const_flag),
                            Operand::Reg(val_reg),
                            Operand::Imm(uv as u16),
                        ));
                    } else if let Some(cell_idx) = captured_cell {
                        ctx.inst(Inst::new(
                            OpCode::CELL_SET,
                            Operand::None,
                            Operand::Reg(val_reg),
                            Operand::Imm(cell_idx as u16),
                        ));
                    } else {
                        let is_const = ctx.lookup_const_flag(name);
                        let const_flag = if is_const { 1 } else { 0 };
                        let var_reg = ctx.lookup_or_global(name);
                        // 全局不可写内置槽：strict 抛 TypeError（put 失败）；sloppy 跳过槽写，
                        // 表达式值 = RHS（尾部结果装载取 val_reg）。短路时序与 PutValue 一致：
                        // 短路未通过不到达此处、不抛错，通过后才在写点拦截。
                        let readonly_builtin = ctx.targets_readonly_builtin(name, var_reg);
                        if readonly_builtin && ctx.is_strict {
                            let _ = self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx)?;
                        } else if !readonly_builtin {
                            // 隐式全局判定以寄存器集合为准：读侧（旧值解析）已登记绑定，
                            // 写侧二次解析命中集合而非"新登记"。
                            let is_implicit = ctx.implicit_global_writes.contains(&var_reg);
                            if is_implicit && ctx.is_strict {
                                // 严格模式未声明写：抛 ReferenceError，后续 LOAD_VAR 不可达。
                                let _ = self.emit_strict_undeclared_write(name, ctx)?;
                            } else {
                                ctx.inst(Inst::new(
                                    OpCode::STORE_VAR,
                                    Operand::Reg(var_reg),
                                    Operand::Reg(val_reg),
                                    Operand::Imm(const_flag),
                                ));
                                if is_implicit {
                                    self.emit_implicit_global_write(name, var_reg, ctx);
                                }
                            }
                        }
                    }
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(val_reg),
                        Operand::None,
                    ));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
                    Ok(result_reg)
                } else if assign.operator == AssignmentOperator::Addition
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
                    let name = id_ref.name.as_str();
                    // with 体内自由标识符的复合赋值由运行时对象遮蔽判定，不静态检查；
                    // 其余路径 TDZ 检查在 RHS 求值之前（规范：赋值引用解析先于 RHS 副作用）。
                    let with_dynamic = !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name);
                    if !with_dynamic {
                        self.emit_identifier_tdz_guard(name, ctx)?;
                    }
                    let rhs = self.emit_expression(&assign.right, ctx)?;
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
                        _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                    };
                    // with 体内自由标识符的复合赋值走动态路径：对象有属性则读对象、
                    // 运算后写回对象，否则回退外层静态复合。
                    if with_dynamic {
                        let obj_reg = ctx.innermost_with_obj().expect("with stack non-empty");
                        let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));

                        let has_reg = ctx.alloc_reg();
                        ctx.inst(Inst::new(
                            OpCode::IN,
                            Operand::Reg(has_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(obj_reg),
                        ));
                        let fallback_label = ctx.next_label_id();
                        let end_label = ctx.next_label_id();
                        ctx.inst(Inst::jmp_if_false(has_reg, fallback_label));

                        // 对象有该属性：读对象属性 → 运算 → 写回对象。
                        let val_reg = ctx.alloc_reg();
                        ctx.inst(Inst::new(
                            OpCode::GET_PROP_DYNAMIC,
                            Operand::Reg(obj_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(val_reg),
                        ));
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                        ctx.inst(Inst::new(
                            OpCode::SET_PROP_DYNAMIC,
                            Operand::Reg(obj_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(val_reg),
                        ));
                        ctx.inst(Inst::jmp(end_label));

                        // 对象无该属性：回退静态复合赋值，结果写入同一 val_reg。
                        ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
                        let fallback_val = self.emit_compound_identifier_static(name, op, rhs, ctx)?;
                        ctx.inst(Inst::new(
                            OpCode::LOAD_VAR,
                            Operand::Reg(val_reg),
                            Operand::Reg(fallback_val),
                            Operand::None,
                        ));
                        ctx.labels.set_label_pos(end_label, ctx.insts.len());
                        return Ok(val_reg);
                    }
                    self.emit_compound_identifier_static(name, op, rhs, ctx)
                } else {
                    Err(format!("compound assignment operator {:?} not supported", assign.operator))
                }
            } else {
                let name = id_ref.name.as_str();
                // with 体内自由标识符走动态写（对象属性运行时遮蔽判定，不静态检查 TDZ）；
                // 其余路径 TDZ 检查在 RHS 求值之前（规范：赋值引用解析先于 RHS 副作用）。
                let with_dynamic = !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name);
                if !with_dynamic {
                    self.emit_identifier_tdz_guard(name, ctx)?;
                }
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                // 赋值函数/箭头/class 表达式 → 推断 name。
                if crate::is_anonymous_function_definition(&assign.right) {
                    if let Some(sub_mod) = ctx.nested.last_mut() {
                        if sub_mod.function_name.is_none() {
                            sub_mod.function_name = Some(name.to_string());
                        }
                    }
                }
                if with_dynamic {
                    let is_const = ctx.lookup_const_flag(name);
                    let const_flag: u16 = if is_const { 1 } else { 0 };
                    self.emit_with_dynamic_write(name, val_reg, const_flag, ctx);
                    return Ok(val_reg);
                }
                let is_const = ctx.lookup_const_flag(name);
                let const_flag: u16 = if is_const { 1 } else { 0 };
                self.emit_identifier_store(name, val_reg, const_flag, ctx);
                Ok(val_reg)
            }
        } else if matches!(
            &assign.left,
            oxide_parser::AssignmentTarget::ArrayAssignmentTarget(_)
                | oxide_parser::AssignmentTarget::ObjectAssignmentTarget(_)
        ) {
            if assign.operator != AssignmentOperator::Assign {
                return Err("compound destructuring assignment not supported".into());
            }
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            self.emit_assign_target(&assign.left, val_reg, ctx)?;
            Ok(val_reg)
        } else {
            Err("assignment target not supported".into())
        }
    }

    /// 计算成员赋值的动态键路径：键运行期求值（非常量键）时经 DYNAMIC 指令读写。
    ///
    /// # 步骤
    /// 1. 求值对象与键表达式到寄存器。
    /// 2. 逻辑赋值：读属性 → 短路测试 → 条件写回；复合赋值：读 → 运算 → 写回；
    ///    普通赋值直接写回。
    ///
    /// # 注意事项
    /// - 键表达式有 ToPropertyKey 副作用或非常量时才调用（常量字符串键走 IC 折叠路径）。
    fn emit_computed_member_dynamic(
        &self, assign: &oxide_parser::AssignmentExpression, member: &oxide_parser::ComputedMemberExpression,
        ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if let Some(logical_op) = assign.operator.to_logical_operator() {
            let store_label = ctx.next_label_id();
            let end_label = ctx.next_label_id();
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let key_reg = self.emit_expression(&member.expression, ctx)?;
            let result_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(
                OpCode::GET_PROP_DYNAMIC,
                Operand::Reg(obj_reg),
                Operand::Reg(key_reg),
                Operand::Reg(result_reg),
            ));
            self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
            ctx.labels.set_label_pos(store_label, ctx.insts.len());
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            ctx.inst(Inst::new(
                OpCode::SET_PROP_DYNAMIC,
                Operand::Reg(obj_reg),
                Operand::Reg(key_reg),
                Operand::Reg(val_reg),
            ));
            ctx.inst(Inst::new(
                OpCode::LOAD_VAR,
                Operand::Reg(result_reg),
                Operand::Reg(val_reg),
                Operand::None,
            ));
            ctx.labels.set_label_pos(end_label, ctx.insts.len());
            return Ok(result_reg);
        }
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        if assign.operator != AssignmentOperator::Assign {
            // 复合赋值：读属性 → 运算 → 写回。val_reg 同时承载旧值与新值。
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(
                OpCode::GET_PROP_DYNAMIC,
                Operand::Reg(obj_reg),
                Operand::Reg(key_reg),
                Operand::Reg(val_reg),
            ));
            let rhs = self.emit_expression(&assign.right, ctx)?;
            let op = match assign.operator {
                AssignmentOperator::Addition => OpCode::ADD,
                AssignmentOperator::Subtraction => OpCode::SUB,
                AssignmentOperator::Multiplication => OpCode::MUL,
                AssignmentOperator::Division => OpCode::DIV,
                AssignmentOperator::Remainder => OpCode::MOD,
                // 指数无 COMPOUND_* 对应（COMPOUND_EXP 只读 [Rd,A] 会忽略 rhs）：
                // 走三寄存器 EXP，旧值在 a 槽（val_reg 兼 rd/a）、rhs 在 b 槽。
                AssignmentOperator::Exponential => OpCode::EXP,
                AssignmentOperator::BitwiseAnd => OpCode::BIT_AND,
                AssignmentOperator::BitwiseOR => OpCode::BIT_OR,
                AssignmentOperator::BitwiseXOR => OpCode::BIT_XOR,
                AssignmentOperator::ShiftLeft => OpCode::SHL,
                AssignmentOperator::ShiftRight => OpCode::SHR,
                AssignmentOperator::ShiftRightZeroFill => OpCode::USHR,
                _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
            };
            ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(rhs)));
            ctx.inst(Inst::new(
                OpCode::SET_PROP_DYNAMIC,
                Operand::Reg(obj_reg),
                Operand::Reg(key_reg),
                Operand::Reg(val_reg),
            ));
            return Ok(val_reg);
        }
        let val_reg = self.emit_expression(&assign.right, ctx)?;
        ctx.inst(Inst::new(
            OpCode::SET_PROP_DYNAMIC,
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            Operand::Reg(val_reg),
        ));
        Ok(val_reg)
    }
}
