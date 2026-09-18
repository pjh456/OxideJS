//! 标识符表达式 emit：按绑定/builtin/全局解析寄存器。
//!
//! with 语句体内，自由标识符（非 with 内部声明的绑定）编译为动态解析：
//! 运行时先查 with 对象是否有该属性，命中则取属性值，否则回退静态解析。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

impl Emitter {
    /// 静态标识符读取：可重赋依赖导入活读 / upvalue / 被捕获 cell / 全局槽。
    ///
    /// # 注意事项
    /// - 活读映射仅模块顶层 ctx 填充：嵌套函数内读 import 名仍走链接期快照
    ///   （`current_upvalue_captures` 不在此列，闭包内不继承映射）。
    fn emit_static_identifier_read(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 可重赋依赖的命名/默认导入：读点直接查依赖命名空间当前值，跟随源模块重赋。
        if let Some((dep_ns, exported)) = ctx.module_live_imports.get(name).cloned() {
            let name_reg = self.load_string_const(&exported, ctx);
            return self.emit_module_call(ctx, "__moduleGet", &[dep_ns, name_reg]);
        }

        for (uv_idx, up) in ctx.current_upvalue_captures.iter().enumerate() {
            if up.name == name {
                let r = ctx.alloc_reg();
                let idx = uv_idx as u8;
                ctx.inst(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(r), Operand::Imm(idx as u16), Operand::None));
                return Ok(r);
            }
        }

        // 捕获集是函数级名字并集：块级 let/const 的 cell 在块退出后仍存留，但名字
        // 已不在作用域链内。仅当名字当前可解析为真实词法绑定时才走 cell（隐式全局
        // 登记不算），否则落下方全局解析（未声明读经 LOAD_GLOBAL 抛 ReferenceError），
        // 不得按名误读已失效的 cell。
        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            if let Some(binding_reg) = ctx.visible_binding_reg(name) {
                let r = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(r),
                    Operand::Reg(binding_reg),
                    Operand::Imm(cell_idx as u16),
                ));
                return Ok(r);
            }
        }

        let var_reg = match ctx.lookup_or_builtin(name) {
            Ok(reg) => reg,
            Err(err) if err.contains("before initialization") => {
                // TDZ：块级预声明占位后，声明点前读取编译为运行时抛 ReferenceError。
                return self.emit_tdz_throw(&err, ctx);
            }
            Err(err) => return Err(err),
        };
        let r = ctx.alloc_reg();
        // 顶层已声明 var 裸读与未声明标识符读同形：读全局对象属性（顶层 var 的唯一
        // 存储，引擎侧不保留镜像副本，见 `compile_ctx.rs` 的 `global_tier_names` 字段
        // 文档），缺失抛 ReferenceError（unresolvable）。tier 名属性由
        // GlobalDeclarationInstantiation（脚本顶层声明实例化）序言创建（仅删除后缺失）；
        // 未声明名属性缺失即 unresolvable。已声明 var 不再落引擎侧镜像副本。
        // 隐式全局槽（读侧登记与写侧登记同属一个槽）同样读全局对象属性：属性是
        // 隐式全局值的唯一真源，嵌套函数内 delete 真删后外层裸读据此可见，引擎侧
        // 镜像槽不反映删除。
        if self.is_global_tier_name(ctx, name) || ctx.is_implicit_global_reg(var_reg) {
            let key_idx = ctx.add_constant(Constant::String(name.to_string()));
            ctx.inst(Inst::new(OpCode::LOAD_GLOBAL, Operand::Reg(r), Operand::Const(key_idx), Operand::None));
        } else {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r), Operand::Reg(var_reg), Operand::None));
        }
        Ok(r)
    }

    /// with 内动态读取：先查 with 对象属性，无则回退静态解析。
    ///
    /// # 边界与前提
    /// - 仅在 `with_stack` 非空且名字非 with 内部绑定时调用。
    /// - 对象属性存在性用 `in` 判定（含原型链），与对象环境记录的 HasBinding 一致。
    fn emit_with_dynamic_read(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        let obj_reg = ctx.innermost_with_obj().expect("with stack non-empty");
        let key_idx = ctx.add_constant(Constant::String(name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));

        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::IN, Operand::Reg(has_reg), Operand::Reg(key_reg), Operand::Reg(obj_reg)));

        let fallback_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(has_reg, fallback_label));

        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::GET_PROP_DYNAMIC,
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            Operand::Reg(result_reg),
        ));
        ctx.inst(Inst::jmp(end_label));

        ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
        // 回退：upvalue / 被捕获 cell / 静态绑定 / 未定义四选一。
        // 对象无该属性时才走此处，语义与静态解析一致。
        if let Some((uv_idx, _)) = ctx.current_upvalue_captures.iter().enumerate().find(|(_, u)| u.name == name) {
            ctx.inst(Inst::new(
                OpCode::LOAD_UPVALUE,
                Operand::Reg(result_reg),
                Operand::Imm(uv_idx as u16),
                Operand::None,
            ));
        } else if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            // 回退分支的捕获 cell 同样只在名字当前可解析为真实词法绑定时才有效；
            // 否则与静态解析一致：顶层 tier 名读全局对象属性，其余读 undefined
            // （with 回退不抛未解析引用）。
            if let Some(binding_reg) = ctx.visible_binding_reg(name) {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(result_reg),
                    Operand::Reg(binding_reg),
                    Operand::Imm(cell_idx as u16),
                ));
            } else if self.is_global_tier_name(ctx, name) {
                let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                ctx.inst(Inst::new(
                    OpCode::LOAD_GLOBAL,
                    Operand::Reg(result_reg),
                    Operand::Const(key_idx),
                    Operand::None,
                ));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                ctx.inst(Inst::load_const(Operand::Reg(result_reg), undef_idx));
            }
        } else if self.is_global_tier_name(ctx, name) {
            // 顶层已声明 var：with 对象无该属性时回退读全局对象属性（顶层 var 的唯一
            // 存储，引擎侧不保留镜像副本）。
            let key_idx = ctx.add_constant(Constant::String(name.to_string()));
            ctx.inst(Inst::new(
                OpCode::LOAD_GLOBAL,
                Operand::Reg(result_reg),
                Operand::Const(key_idx),
                Operand::None,
            ));
        } else if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::Reg(reg), Operand::None));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::Reg(result_reg), undef_idx));
        }
        ctx.labels.set_label_pos(end_label, ctx.insts.len());

        Ok(result_reg)
    }

    /// 静态标识符写入：upvalue / 被捕获 cell / 普通槽（含 const guard）。
    ///
    /// # 边界与前提
    /// - `const_flag` 为 1 时编译期直接抛 TypeError（值无关），覆盖普通/upvalue/cell 全写路径；
    ///   不发射写指令（THROW 后不可达）。
    /// - cell 写穿共享单元，无运行时 guard；const 拦截由本入口编译期完成。
    /// - 不可写全局内置（undefined/NaN/Infinity）的全局绑定写在此拦截：sloppy 跳过
    ///   寄存器写（槽保留入口预载原值），strict 抛 TypeError；局部遮蔽绑定不受影响。
    ///
    /// # 副作用
    /// - live 模块顶层对导出源绑定的写入追加 `__moduleSet`，同步命名空间条目。
    pub(crate) fn emit_identifier_store(
        &self, name: &str, val_reg: u32, const_flag: u16, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        // 循环 update 段的 let/const 循环变量是 per-iteration 可变绑定（CreateMutableBinding），
        // 写寄存器而非 cell，且豁免 const 检查（register_update_names 覆盖全部循环头声明名）。
        let in_loop_update = ctx.register_update_names.iter().any(|n| n == name);
        // const 再赋值编译期抛 TypeError：赋值路径（含闭包捕获 const 写 cell）统一拦截。
        if const_flag != 0 && !in_loop_update {
            let _ = self.emit_throw_error("TypeError", "Assignment to constant variable", ctx)?;
            return Ok(());
        }
        // 循环 update 段：被捕获绑定走寄存器而非 cell——C 风格 for 的 let/const
        // 循环变量每迭代新分配一个 cell，update 写寄存器供其拷入，不污染本迭代
        // 闭包捕获的 cell（机制见 `compile_ctx.rs` 的 `register_update_names` 字段文档）。
        if in_loop_update {
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                if ctx.targets_readonly_builtin(name, reg) {
                    // 全局不可写内置：put 永不成功——sloppy 静默跳过，strict 抛错。
                    if ctx.is_strict {
                        let _ = self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
                    }
                    return Ok(());
                }
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(reg), Operand::Reg(val_reg), Operand::Imm(0)));
                // 可写内置名：值同步落全局对象属性，裸读（镜像）与反射不失步。
                if ctx.targets_writable_builtin(name, reg) {
                    self.emit_global_put_write(name, reg, ctx);
                }
                return Ok(());
            }
        }
        // 目标若是 upvalue 引用，走 STORE_UPVALUE
        if let Some(uv_idx) = ctx.current_upvalue_captures.iter().position(|u| u.name == name) {
            ctx.inst(Inst::new(
                OpCode::STORE_UPVALUE,
                Operand::Imm(const_flag),
                Operand::Reg(val_reg),
                Operand::Imm(uv_idx as u16),
            ));
            self.emit_module_write_through(name, val_reg, ctx)?;
            return Ok(());
        }
        // 目标若是被捕获 cell，走 CELL_SET；仅当名字当前可解析为真实词法绑定时才写
        // cell——捕获集按名保留的块级绑定在块退出后不可解析（隐式全局登记不算），
        // 须落下方全局写（sloppy 物化隐式全局属性，strict 抛 ReferenceError）。
        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            if ctx.visible_binding_reg(name).is_some() {
                ctx.inst(Inst::new(
                    OpCode::CELL_SET,
                    Operand::None,
                    Operand::Reg(val_reg),
                    Operand::Imm(cell_idx as u16),
                ));
                self.emit_module_write_through(name, val_reg, ctx)?;
                return Ok(());
            }
        }
        let var_reg = ctx.lookup_or_global(name);
        if ctx.targets_readonly_builtin(name, var_reg) {
            // 全局不可写内置槽：sloppy 跳过寄存器写（槽保留运行入口预载原值，
            // 静默 no-op）；strict 在 RHS 已求值后抛 TypeError（put 失败）。
            if ctx.is_strict {
                let _ = self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx);
            }
            return Ok(());
        }
        let is_tier = self.is_global_tier_name(ctx, name);
        let is_implicit = ctx.is_implicit_global_reg(var_reg);
        if is_implicit && ctx.is_strict {
            // 严格模式未声明写：发射 ReferenceError 抛错，跳过寄存器写（值无关）。
            self.emit_strict_undeclared_write(name, ctx)?;
            return Ok(());
        }
        if is_tier {
            // 顶层已声明 var 裸写：写入全局对象属性（顶层 var 的唯一存储，引擎侧不保留
            // 镜像副本）。
            self.emit_tier_global_write(name, val_reg, ctx);
            return Ok(());
        }
        ctx.inst(Inst::new(
            OpCode::STORE_VAR,
            Operand::Reg(var_reg),
            Operand::Reg(val_reg),
            Operand::Imm(const_flag),
        ));
        if is_implicit {
            self.emit_implicit_global_write(name, var_reg, ctx);
        } else if ctx.targets_writable_builtin(name, var_reg) {
            // 可写内置名：值同步写入全局对象属性——`DEFINE_GLOBAL_PROP_C`（`0x98`，
            // 见 `crates/oxide_bytecode/src/opcode.rs`）对既有可写属性仅更新值、不改变
            // enumerable/configurable 位；否则内置名镜像槽与 globalThis 反射读到不同值。
            self.emit_global_put_write(name, var_reg, ctx);
        }
        // 模块顶层对导出源绑定的写入同步命名空间条目；非 live 模块空表直接返回。
        self.emit_module_write_through(name, val_reg, ctx)?;
        Ok(())
    }

    /// with 内动态写入：对象有该属性则写对象，否则回退静态写入。
    ///
    /// # 边界与前提
    /// - 仅在 `with_stack` 非空且名字非 with 内部绑定时调用。
    /// - 对象属性存在性判定与动态读取一致（`in` 含原型链）。
    /// - 回退目标未在静态作用域声明时不登记全局（with 外不应可见）。
    pub(crate) fn emit_with_dynamic_write(
        &self, name: &str, val_reg: u32, const_flag: u16, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let obj_reg = ctx.innermost_with_obj().expect("with stack non-empty");
        let key_idx = ctx.add_constant(Constant::String(name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));

        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::IN, Operand::Reg(has_reg), Operand::Reg(key_reg), Operand::Reg(obj_reg)));

        let fallback_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(has_reg, fallback_label));

        ctx.inst(Inst::new(
            OpCode::SET_PROP_DYNAMIC,
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            Operand::Reg(val_reg),
        ));
        ctx.inst(Inst::jmp(end_label));

        ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
        // 回退只写静态作用域已声明的绑定；未声明时丢弃值（隐式全局在 with 外不可解析）。
        if ctx.scopes.symbols.lookup_any(name).is_some() {
            self.emit_identifier_store(name, val_reg, const_flag, ctx)?;
        }
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        Ok(())
    }

    pub(crate) fn emit_identifier_expression(
        &self, ident: &oxide_parser::IdentifierReference, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let name = ident.name.as_str();

        // with 体内的自由标识符走动态解析（对象属性优先，回退外层）。
        if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
            return self.emit_with_dynamic_read(name, ctx);
        }

        self.emit_static_identifier_read(name, ctx)
    }
}
