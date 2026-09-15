//! 运算符域：二元/一元/条件/逻辑/更新/`in` 表达式 emit 与分发。
//! 逻辑与 `??` 用短路跳转，复合求值利用 `is_side_effect_free` 优化。
//! 函数：`emit_operator` 及各类 `emit_*_expression`。

use crate::{is_side_effect_free, BinaryOperator, CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ChainElement, Expression, LogicalOperator, SimpleAssignmentTarget, UnaryOperator, UpdateOperator};

impl Emitter {
    fn emit_private_in_expression(
        &self, pin: &oxide_parser::PrivateInExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let obj_reg = self.emit_expression(&pin.right, ctx)?;
        let key_reg = self.emit_private_id_reg(pin.left.name.as_str(), ctx)?;
        let (brand_reg, brand_id) = self.private_access_brand(obj_reg, pin.left.name.as_str(), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::private_brand_in(
            Operand::Reg(result_reg),
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            brand_reg,
            brand_id,
        ));
        Ok(result_reg)
    }

    fn emit_binary_expression(
        &self, bin: &oxide_parser::BinaryExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // Addition 左结合链摊平：≥3 操作数合并为单条 CONCAT_N（单趟预分配拼接，
        // 消除 N-2 次中间串分配）。只下钻 left 链、right 括号不拆（a+(b+c) 退化 ADD），
        // 保 f64 结合性与源码求值序。
        if bin.operator == BinaryOperator::Addition {
            let mut operands = Vec::new();
            collect_add_operands(&bin.left, &mut operands);
            operands.push(&bin.right);
            // 上限防寄存器压力：CONCAT_N 单点读全部操作数（须同时存活），超长链回退
            // 左结合 ADD（寄存器随链复用，纯表达式寄存器占用有界）。
            if operands.len() >= 3 && operands.len() <= MAX_CONCAT_N_OPERANDS {
                let mut regs = Vec::with_capacity(operands.len());
                for op in &operands {
                    regs.push(self.emit_expression(op, ctx)?);
                }
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::concat_n(Operand::Reg(result_reg), &regs));
                return Ok(result_reg);
            }
        }
        let left = self.emit_expression(&bin.left, ctx)?;
        let right = self.emit_expression(&bin.right, ctx)?;
        let op = match bin.operator {
            BinaryOperator::Addition => OpCode::ADD,
            BinaryOperator::Subtraction => OpCode::SUB,
            BinaryOperator::Multiplication => OpCode::MUL,
            BinaryOperator::Division => OpCode::DIV,
            BinaryOperator::Remainder => OpCode::MOD,
            BinaryOperator::Exponential => OpCode::EXP,
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
        };
        ctx.inst(Inst::new(op, Operand::Reg(left), Operand::Reg(left), Operand::Reg(right)));
        Ok(left)
    }

    fn emit_unary_expression(&self, un: &oxide_parser::UnaryExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        if matches!(un.operator, UnaryOperator::Delete) {
            // 括号的引用性透传内层操作数：先逐层剥括号，再按内层形态分派。
            let mut operand = &un.argument;
            while let Expression::ParenthesizedExpression(paren) = operand {
                operand = &paren.expression;
            }
            return self.emit_delete_operand(operand, ctx);
        }
        let arg = if matches!(un.operator, UnaryOperator::Typeof) {
            self.emit_typeof_operand(&un.argument, ctx)?
        } else {
            self.emit_expression(&un.argument, ctx)?
        };
        match un.operator {
            UnaryOperator::UnaryNegation => {
                ctx.inst(Inst::new(OpCode::NEG, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Typeof => {
                ctx.inst(Inst::new(OpCode::TYPEOF, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Void => {
                ctx.inst(Inst::new(OpCode::VOID, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::LogicalNot => {
                ctx.inst(Inst::new(OpCode::NOT, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::BitwiseNot => {
                ctx.inst(Inst::new(OpCode::BIT_NOT, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::UnaryPlus => {
                ctx.inst(Inst::new(OpCode::UNARY_PLUS, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Delete => Err("invalid delete target".into()),
        }
    }

    /// delete 操作数求值：引用形态（标识符/静态成员/计算成员/链式）按引用语义
    /// 删除并返回删除结果；非引用操作数返回 true。
    ///
    /// # 步骤
    /// 1. 标识符：with 体动态解析、可删全局内置镜像槽先行；其余按 DeleteBinding
    ///    三分类（见标识符臂注释）——局部绑定与脚本自身顶层已声明名发 false
    ///    常数，未声明名/隐式全局槽/eval 程序自身顶层已声明名发全局对象运行期
    ///    探针。严格模式的 delete 标识符是早期错误，由语义分析阶段拦截，不在此出现。
    /// 2. 静态/计算成员与链式成员：从对象真删属性，结果为删除返回值。
    /// 3. 其余形态（字面量/调用/new/this/一元形/序列/嵌套 delete 等）：先求值
    ///    操作数（副作用与操作数求值期错误不被吞），再发常量 true。
    fn emit_delete_operand(&self, operand: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match operand {
            Expression::Identifier(ident) => {
                let name = ident.name.as_str();
                // with 体内自由标识符：对象有该属性则删除对象属性（返回删除结果），
                // 否则非严格语义返回 true。
                if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
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
                    // 复制 obj 到临时寄存器执行删除：DELETE_PROP_DYNAMIC 把结果写回
                    // rd 槽，直接用它会把 with 对象寄存器覆盖为布尔值。
                    let tmp_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(tmp_reg), Operand::Reg(obj_reg), Operand::None));
                    ctx.inst(Inst::new(
                        OpCode::DELETE_PROP_DYNAMIC,
                        Operand::Reg(tmp_reg),
                        Operand::Reg(tmp_reg),
                        Operand::Reg(key_reg),
                    ));
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(tmp_reg),
                        Operand::None,
                    ));
                    ctx.inst(Inst::jmp(end_label));
                    ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
                    let true_idx = ctx.add_constant(Constant::Boolean(true));
                    ctx.inst(Inst::load_const(Operand::Reg(result_reg), true_idx));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
                    return Ok(result_reg);
                }
                // 可删全局内置（可写全局名除宿主名 $262）：运行期真删
                // 全局对象属性（c:true 数据描述符）并返 true；删除成功时清镜像
                // 槽，裸读与 globalThis 反射不失步。
                if let Some(slot_reg) = ctx.global_builtin_delete_slot(name) {
                    let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                    let reg = ctx.alloc_reg();
                    ctx.inst(Inst::delete_global_prop_c(Operand::Reg(reg), Operand::Reg(slot_reg), key_idx));
                    return Ok(reg);
                }
                // DeleteBinding 三分类（13.5.1.2 步5 绑定引用走 base.DeleteBinding）：
                // - 局部绑定（捕获 cell/upvalue/lookup 命中函数或块作用域槽）与
                //   非 eval 脚本自身顶层已声明名（脚本 var/函数名 c:false）→ false 常数；
                // - 其余——未声明名（引用不可解析）、隐式全局槽、eval 程序自身顶层
                //   已声明名（c:true）——发全局对象运行期探针（与 Reflect.deleteProperty
                //   同源 DeleteBinding 语义：缺失 → true；不可配置 → false 且保留；
                //   可配置 → 真删且 true）。三类全局属性的 c 位已由各自写点物化，
                //   探针取值即规范值；独立编译的 eval 程序见不到调用方域变量，静态
                //   "当前程序内是否声明"粗于规范动态判定，故不可解析面一律运行期定值。
                //   严格模式的 delete 标识符由语义分析提前拦截为早期错误，本臂在
                //   strict 代码不可达。
                let local = ctx.captured_bindings.contains_key(name)
                    || ctx.current_upvalue_captures.iter().any(|u| u.name == name);
                let probe = if local {
                    false
                } else {
                    match ctx.scopes.symbols.lookup_any_binding(name) {
                        // 未声明名（引用不可解析）：运行期按全局属性定值。
                        None => true,
                        Some((binding, scope_idx)) => {
                            if scope_idx == 0 {
                                // 全局作用域：隐式全局槽（未声明名读写登记，属性
                                // c:true）与 eval 程序自身顶层名（c:true）可删；
                                // 非 eval 脚本自身顶层 var/函数名（c:false）保留 false。
                                ctx.is_implicit_global_reg(binding.reg) || ctx.is_eval_script
                            } else {
                                // 函数/块作用域局部绑定：非属性引用，恒 false。
                                false
                            }
                        }
                    }
                };
                if probe {
                    let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                    let reg = ctx.alloc_reg();
                    ctx.inst(Inst::delete_global_prop_c(Operand::Reg(reg), Operand::None, key_idx));
                    // 隐式全局槽被真删后，同程序后续裸读该名须走全局对象属性
                    // （A 侧单一真值）：缺失属性读抛 ReferenceError，delete 的
                    // 真删效应在读侧可见。
                    if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                        if ctx.is_implicit_global_reg(binding.reg) {
                            ctx.implicit_global_reads.insert(binding.reg);
                        }
                    }
                    return Ok(reg);
                }
                let idx = ctx.add_constant(Constant::Boolean(false));
                let reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
                Ok(reg)
            }
            Expression::StaticMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                ctx.inst(Inst::delete_prop_static(Operand::Reg(obj_reg), const_idx as u32));
                Ok(obj_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                ctx.inst(Inst::new(
                    OpCode::DELETE_PROP_DYNAMIC,
                    Operand::Reg(obj_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                ));
                Ok(obj_reg)
            }
            Expression::ChainExpression(chain) => {
                let short_label = ctx.next_label_id();
                let result_reg = match &chain.expression {
                    ChainElement::StaticMemberExpression(member) => {
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        if member.optional {
                            let dup_reg = ctx.alloc_reg();
                            ctx.inst(Inst::new(
                                OpCode::LOAD_VAR,
                                Operand::Reg(dup_reg),
                                Operand::Reg(obj_reg),
                                Operand::None,
                            ));
                            ctx.inst(Inst::jmp_if_nullish(dup_reg, short_label));
                        }
                        let prop_name = member.property.name.as_str();
                        let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        ctx.inst(Inst::delete_prop_static(Operand::Reg(obj_reg), const_idx as u32));
                        obj_reg
                    }
                    ChainElement::ComputedMemberExpression(member) => {
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        if member.optional {
                            let dup_reg = ctx.alloc_reg();
                            ctx.inst(Inst::new(
                                OpCode::LOAD_VAR,
                                Operand::Reg(dup_reg),
                                Operand::Reg(obj_reg),
                                Operand::None,
                            ));
                            ctx.inst(Inst::jmp_if_nullish(dup_reg, short_label));
                        }
                        let key_reg = self.emit_expression(&member.expression, ctx)?;
                        ctx.inst(Inst::new(
                            OpCode::DELETE_PROP_DYNAMIC,
                            Operand::Reg(obj_reg),
                            Operand::Reg(obj_reg),
                            Operand::Reg(key_reg),
                        ));
                        obj_reg
                    }
                    _ => return Err("invalid delete target".into()),
                };
                let end_label = ctx.next_label_id();
                ctx.inst(Inst::jmp(end_label));
                ctx.labels.set_label_pos(short_label, ctx.insts.len());
                let true_idx = ctx.add_constant(Constant::Boolean(true));
                ctx.inst(Inst::load_const(Operand::Reg(result_reg), true_idx));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                Ok(result_reg)
            }
            _ => {
                // 非引用操作数恒 true，但操作数本身须先求值：副作用与操作数
                // 求值期错误（如未声明基引用）不被吞。
                let _ = self.emit_expression(operand, ctx)?;
                let idx = ctx.add_constant(Constant::Boolean(true));
                let reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
                Ok(reg)
            }
        }
    }

    /// typeof 操作数求值：未声明标识符特判为 "undefined"。
    ///
    /// # 步骤
    /// 1. 剥括号后若为标识符且不在任何作用域/闭包捕获中、非 with 动态解析，
    ///    发射 LOAD_GLOBAL_TYPEOF：运行期查 global object 属性，命中取真实值
    ///    （含不在 BUILTIN_GLOBALS 名单的真实全局，如 Temporal），缺失求值
    ///    undefined（IsUnresolvableReference 语义）。
    /// 2. 其余（TDZ 绑定、已声明、with 内）走正常求值：TDZ 抛 ReferenceError、
    ///    with 动态回退未定义、已声明读真实值。
    ///
    /// # 边界与前提
    /// - 未声明标识符不能走 LOAD_GLOBAL（读未声明抛 ReferenceError），故必须在此特判；
    /// - `typeof x; var x;` / `typeof x; let x;` 分别由 var 预声明（已初始化→undefined）
    ///   与 TDZ 占位（未初始化→抛）覆盖，不落本分支。
    fn emit_typeof_operand(&self, argument: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        let mut arg_expr = argument;
        while let Expression::ParenthesizedExpression(p) = arg_expr {
            arg_expr = &p.expression;
        }
        if let Expression::Identifier(ident) = arg_expr {
            let name = ident.name.as_str();
            let in_with_dynamic = !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name);
            let captured =
                ctx.current_upvalue_captures.iter().any(|u| u.name == name) || ctx.captured_bindings.contains_key(name);
            // 顶层已声明 var：typeof 读全局对象属性（A 侧单一真值），缺失 → "undefined"
            // （非抛，IsUnresolvableReference 语义）。未声明名同走此路（lookup 未命中）。
            // 隐式全局槽（未声明名读写登记）同走属性路：delete 真删后缺失 → "undefined"
            // 而非经镜像槽读旧值或抛 ReferenceError（typeof 对 unresolvable 引用不抛）。
            let binding = ctx.scopes.symbols.lookup_any_binding(name);
            let implicit_global = binding.is_some_and(|(b, _)| ctx.is_implicit_global_reg(b.reg));
            let is_tier = self.is_global_tier_name(ctx, name);
            if !in_with_dynamic && !captured && (is_tier || binding.is_none() || implicit_global) {
                let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                let r = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::LOAD_GLOBAL_TYPEOF,
                    Operand::Reg(r),
                    Operand::Const(key_idx),
                    Operand::None,
                ));
                return Ok(r);
            }
        }
        self.emit_expression(argument, ctx)
    }

    fn emit_conditional_expression(
        &self, cond: &oxide_parser::ConditionalExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let else_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();

        let test_reg = self.emit_expression(&cond.test, ctx)?;
        ctx.inst(Inst::jmp_if_false(test_reg, else_label));

        let cons_reg = self.emit_expression(&cond.consequent, ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(result_reg),
            Operand::Reg(cons_reg),
            Operand::None,
        ));

        ctx.inst(Inst::jmp(end_label));

        ctx.labels.set_label_pos(else_label, ctx.insts.len());
        let alt_reg = self.emit_expression(&cond.alternate, ctx)?;
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(result_reg),
            Operand::Reg(alt_reg),
            Operand::None,
        ));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());

        Ok(result_reg)
    }

    fn emit_logical_expression(
        &self, log: &oxide_parser::LogicalExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let left_reg = self.emit_expression(&log.left, ctx)?;

        if is_side_effect_free(&log.left) && is_side_effect_free(&log.right) {
            let right_reg = self.emit_expression(&log.right, ctx)?;
            let r = ctx.alloc_reg();
            let op = match log.operator {
                LogicalOperator::And => OpCode::AND,
                LogicalOperator::Or => OpCode::OR,
                LogicalOperator::Coalesce => OpCode::NULLISH,
            };
            ctx.inst(Inst::new(op, Operand::Reg(r), Operand::Reg(left_reg), Operand::Reg(right_reg)));
            return Ok(r);
        }

        if matches!(log.operator, LogicalOperator::Coalesce) {
            let rhs_label = ctx.next_label_id();
            let end_label = ctx.next_label_id();
            let dup_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(left_reg), Operand::None));
            ctx.inst(Inst::jmp_if_nullish(dup_reg, rhs_label));
            ctx.inst(Inst::jmp(end_label));
            ctx.labels.set_label_pos(rhs_label, ctx.insts.len());
            let right_reg = self.emit_expression(&log.right, ctx)?;
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(right_reg), Operand::None));
            ctx.labels.set_label_pos(end_label, ctx.insts.len());
            return Ok(dup_reg);
        }

        let skip_label = match log.operator {
            LogicalOperator::And => ctx.next_label_id(),
            LogicalOperator::Or => ctx.next_label_id(),
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
        };
        let dup_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(left_reg), Operand::None));

        match log.operator {
            LogicalOperator::And => ctx.inst(Inst::jmp_if_false(dup_reg, skip_label)),
            LogicalOperator::Or => ctx.inst(Inst::jmp_if_true(dup_reg, skip_label)),
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
        }

        let right_reg = self.emit_expression(&log.right, ctx)?;
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(right_reg), Operand::None));
        ctx.labels.set_label_pos(skip_label, ctx.insts.len());

        Ok(dup_reg)
    }

    fn emit_update_expression(
        &self, update: &oxide_parser::UpdateExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                // with 体内自由标识符的自增/自减：对象有属性则读写对象，否则回退外层。
                if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
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

                    // 对象有该属性：读属性 → 增减 → 写回对象。
                    let old_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::GET_PROP_DYNAMIC,
                        Operand::Reg(obj_reg),
                        Operand::Reg(key_reg),
                        Operand::Reg(old_reg),
                    ));
                    let new_reg = ctx.alloc_reg();
                    let one_idx = ctx.add_constant(Constant::Int(1));
                    let one_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));
                    let op = if update.operator == UpdateOperator::Increment {
                        OpCode::ADD
                    } else {
                        OpCode::SUB
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(new_reg), Operand::Reg(old_reg), Operand::Reg(one_reg)));
                    ctx.inst(Inst::new(
                        OpCode::SET_PROP_DYNAMIC,
                        Operand::Reg(obj_reg),
                        Operand::Reg(key_reg),
                        Operand::Reg(new_reg),
                    ));
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(if update.prefix { new_reg } else { old_reg }),
                        Operand::None,
                    ));
                    ctx.inst(Inst::jmp(end_label));

                    // 对象无该属性：回退静态自增/自减，结果写入同一 result_reg。
                    ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
                    let static_result = self.emit_identifier_update_static(name, update, ctx)?;
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(static_result),
                        Operand::None,
                    ));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
                    return Ok(result_reg);
                }
                // 判断目标是 upvalue 还是被捕获 cell
                self.emit_identifier_update_static(name, update, ctx)
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let key_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                let val_reg = ctx.alloc_reg();
                let op = match update.operator {
                    UpdateOperator::Increment => OpCode::MEMBER_INC,
                    UpdateOperator::Decrement => OpCode::MEMBER_DEC,
                };
                match op {
                    OpCode::MEMBER_INC => {
                        ctx.inst(Inst::member_inc(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    }
                    OpCode::MEMBER_DEC => {
                        ctx.inst(Inst::member_dec(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    }
                    _ => unreachable!(),
                }
                Ok(val_reg)
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                // 常量字符串键折叠为 IC 静态路径：MEMBER_INC/DEC 携带 IC 扩展字。
                if let Some(key) = crate::expr::member::computed_const_key(&member.expression) {
                    let idx = ctx.add_constant(Constant::String(key));
                    let key_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                    let val_reg = ctx.alloc_reg();
                    let op = match update.operator {
                        UpdateOperator::Increment => OpCode::MEMBER_INC,
                        UpdateOperator::Decrement => OpCode::MEMBER_DEC,
                    };
                    match op {
                        OpCode::MEMBER_INC => {
                            ctx.inst(Inst::member_inc(
                                Operand::Reg(obj_reg),
                                Operand::Reg(val_reg),
                                Operand::Reg(key_reg),
                            ));
                        }
                        OpCode::MEMBER_DEC => {
                            ctx.inst(Inst::member_dec(
                                Operand::Reg(obj_reg),
                                Operand::Reg(val_reg),
                                Operand::Reg(key_reg),
                            ));
                        }
                        _ => unreachable!(),
                    }
                    return Ok(val_reg);
                }
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let val_reg = ctx.alloc_reg();
                let op = match update.operator {
                    UpdateOperator::Increment => OpCode::DYN_MEMBER_INC,
                    UpdateOperator::Decrement => OpCode::DYN_MEMBER_DEC,
                };
                ctx.inst(Inst::new(op, Operand::Reg(obj_reg), Operand::Reg(key_reg), Operand::Reg(val_reg)));
                Ok(val_reg)
            }
            _ => Err("member update not yet supported".into()),
        }
    }

    /// 标识符自增/自减的静态路径：upvalue / 被捕获 cell 走显式读-增减-写回，
    /// 普通槽用 INC_PRE/POST 或 DEC_PRE/POST。返回结果寄存器。
    ///
    /// # 边界与前提
    /// - 不可写全局内置槽：值照算（前缀返回新值、后缀返回旧值）但跳过槽写；
    ///   strict 抛 TypeError。局部遮蔽绑定不受影响。
    fn emit_identifier_update_static(
        &self, name: &str, update: &oxide_parser::UpdateExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // 自增/自减先解析赋值引用：TDZ 绑定抛 ReferenceError；const 抛 TypeError（编译期拦截）。
        // 循环 update 段的 let/const 循环变量是 per-iteration 可变绑定（CreateMutableBinding），
        // 规范允许写，故豁免 const 检查（register_update_names 覆盖全部循环头声明名）。
        self.emit_identifier_tdz_guard(name, ctx)?;
        if !ctx.register_update_names.iter().any(|n| n == name) {
            self.emit_const_write_guard(name, ctx)?;
        }
        let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
        let captured_cell = ctx.captured_bindings.get(name).copied();
        // 循环 update 段：被捕获绑定走寄存器 INC/DEC（C 风格 for 每迭代 fresh，
        // update 写寄存器供下一迭代 fresh 拷贝，不污染本迭代闭包捕获的 cell）。
        if ctx.register_update_names.iter().any(|n| n == name) {
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                let op = match (update.operator, update.prefix) {
                    (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                    (UpdateOperator::Increment, false) => OpCode::INC_POST,
                    (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                    (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
                };
                if ctx.targets_readonly_builtin(name, reg) {
                    if ctx.is_strict {
                        return self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
                    }
                    // 全局不可写内置：值照算（前缀返回新值、后缀返回旧值），跳过槽写。
                    let tmp_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(tmp_reg), Operand::Reg(reg), Operand::None));
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(op, Operand::Reg(tmp_reg), Operand::Reg(result_reg), Operand::Reg(result_reg)));
                    return Ok(result_reg);
                }
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(op, Operand::Reg(reg), Operand::Reg(result_reg), Operand::Reg(result_reg)));
                // 可写内置名：RMW 新值同步落全局对象属性。
                if ctx.targets_writable_builtin(name, reg) {
                    self.emit_global_put_write(name, reg, ctx);
                }
                return Ok(result_reg);
            }
        }
        if let Some(uv) = uv_idx {
            // upvalue：LOAD_UPVALUE + 常量 1 + ADD/SUB + STORE_UPVALUE；
            // 后缀形式返回旧值（前缀返回新值）。
            let old_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(
                OpCode::LOAD_UPVALUE,
                Operand::Reg(old_reg),
                Operand::Imm(uv as u16),
                Operand::None,
            ));
            let one_idx = ctx.add_constant(Constant::Int(1));
            let one_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));
            let op = if update.operator == UpdateOperator::Increment {
                OpCode::ADD
            } else {
                OpCode::SUB
            };
            let new_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(op, Operand::Reg(new_reg), Operand::Reg(old_reg), Operand::Reg(one_reg)));
            let const_flag = if ctx.lookup_const_flag(name) { 1 } else { 0 };
            ctx.inst(Inst::new(
                OpCode::STORE_UPVALUE,
                Operand::Imm(const_flag),
                Operand::Reg(new_reg),
                Operand::Imm(uv as u16),
            ));
            Ok(if update.prefix { new_reg } else { old_reg })
        } else if let Some(cell_idx) = captured_cell {
            // 被捕获 cell：CELL_GET 旧值 + 常量 1 + ADD/SUB + CELL_SET。
            // 后缀形式须保留旧值（结果寄存器），前缀返回新值。
            let old_reg = ctx.alloc_reg();
            if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(old_reg),
                    Operand::Reg(binding.reg),
                    Operand::Imm(cell_idx as u16),
                ));
            } else {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(old_reg),
                    Operand::None,
                    Operand::Imm(cell_idx as u16),
                ));
            }
            let one_idx = ctx.add_constant(Constant::Int(1));
            let one_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));
            let op = if update.operator == UpdateOperator::Increment {
                OpCode::ADD
            } else {
                OpCode::SUB
            };
            let new_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(op, Operand::Reg(new_reg), Operand::Reg(old_reg), Operand::Reg(one_reg)));
            ctx.inst(Inst::new(
                OpCode::CELL_SET,
                Operand::None,
                Operand::Reg(new_reg),
                Operand::Imm(cell_idx as u16),
            ));
            Ok(if update.prefix { new_reg } else { old_reg })
        } else {
            let var_reg = ctx.lookup_or_global(name);
            if ctx.targets_readonly_builtin(name, var_reg) {
                // 全局不可写内置槽：strict 抛 TypeError（put 失败）；sloppy 值照算
                // （前缀返回新值、后缀返回旧值），跳过槽写。
                if ctx.is_strict {
                    return self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
                }
                let tmp_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(tmp_reg), Operand::Reg(var_reg), Operand::None));
                let result_reg = ctx.alloc_reg();
                let op = match (update.operator, update.prefix) {
                    (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                    (UpdateOperator::Increment, false) => OpCode::INC_POST,
                    (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                    (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
                };
                ctx.inst(Inst::new(op, Operand::Reg(tmp_reg), Operand::Reg(result_reg), Operand::Reg(result_reg)));
                return Ok(result_reg);
            }
            let is_tier = self.is_global_tier_name(ctx, name);
            let is_implicit = ctx.is_implicit_global_reg(var_reg);
            if is_implicit && ctx.is_strict {
                // 严格模式未声明更新写：值无关抛 ReferenceError。
                return self.emit_strict_undeclared_write(name, ctx);
            }
            // RMW 前从全局对象属性取旧值：tier 名（顶层已声明 var）与未声明名同，
            // 属性缺失按 undefined（update 旧值角落，GetBaseValue 语义，不抛）。
            if is_tier || is_implicit {
                let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                ctx.inst(Inst::new(
                    OpCode::LOAD_GLOBAL_TYPEOF,
                    Operand::Reg(var_reg),
                    Operand::Const(key_idx),
                    Operand::None,
                ));
            }
            let result_reg = ctx.alloc_reg();
            let op = match (update.operator, update.prefix) {
                (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                (UpdateOperator::Increment, false) => OpCode::INC_POST,
                (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
            };
            ctx.inst(Inst::new(op, Operand::Reg(var_reg), Operand::Reg(result_reg), Operand::Reg(result_reg)));
            if is_tier {
                // 顶层已声明 var 自增/自减：新值落全局对象属性（A 侧单一真值）。
                self.emit_tier_global_write(name, var_reg, ctx);
            } else if is_implicit {
                self.emit_implicit_global_write(name, var_reg, ctx);
            } else if ctx.targets_writable_builtin(name, var_reg) {
                // 可写内置名：RMW 新值同步落全局对象属性。
                self.emit_global_put_write(name, var_reg, ctx);
            }
            Ok(result_reg)
        }
    }

    pub(crate) fn emit_operator(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::BinaryExpression(bin) => self.emit_binary_expression(bin, ctx),
            Expression::PrivateInExpression(pin) => self.emit_private_in_expression(pin, ctx),
            Expression::UnaryExpression(un) => self.emit_unary_expression(un, ctx),
            Expression::ConditionalExpression(cond) => self.emit_conditional_expression(cond, ctx),
            Expression::LogicalExpression(log) => self.emit_logical_expression(log, ctx),
            Expression::UpdateExpression(update) => self.emit_update_expression(update, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}

/// CONCAT_N 摊平的操作数上限：CONCAT_N 单点读全部操作数（须同时存活），寄存器占用
/// = 操作数 + 结果 ≤ 15；超过上限回退左结合 ADD（超长链保持寄存器随链复用属性）。
const MAX_CONCAT_N_OPERANDS: usize = 14;

/// 收集 Addition 左结合链的操作数（源码求值序）：只下钻 left 链，right 恒为单个
/// 操作数（即使自身是 Addition 也不拆——括号右结合保持 f64 结合性不引入偏差）。
fn collect_add_operands<'expr, 'r>(expr: &'r Expression<'expr>, out: &mut Vec<&'r Expression<'expr>>) {
    if let Expression::BinaryExpression(bin) = expr {
        if bin.operator == BinaryOperator::Addition {
            collect_add_operands(&bin.left, out);
            out.push(&bin.right);
            return;
        }
    }
    out.push(expr);
}
