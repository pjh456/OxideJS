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
    /// 静态标识符读取：upvalue / 被捕获 cell / 全局槽。
    fn emit_static_identifier_read(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        for (uv_idx, up) in ctx.current_upvalue_captures.iter().enumerate() {
            if up.name == name {
                let r = ctx.alloc_reg();
                let idx = uv_idx as u8;
                ctx.inst(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(r), Operand::Imm(idx as u16), Operand::None));
                return Ok(r);
            }
        }

        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            let r = ctx.alloc_reg();
            if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(r),
                    Operand::Reg(binding.reg),
                    Operand::Imm(cell_idx as u16),
                ));
            } else {
                ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(r), Operand::None, Operand::Imm(cell_idx as u16)));
            }
            return Ok(r);
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
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r), Operand::Reg(var_reg), Operand::None));
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
            if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(result_reg),
                    Operand::Reg(binding.reg),
                    Operand::Imm(cell_idx as u16),
                ));
            } else {
                ctx.inst(Inst::new(
                    OpCode::CELL_GET,
                    Operand::Reg(result_reg),
                    Operand::None,
                    Operand::Imm(cell_idx as u16),
                ));
            }
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
    pub(crate) fn emit_identifier_store(&self, name: &str, val_reg: u32, const_flag: u16, ctx: &mut CompileCtx) {
        // const 再赋值编译期抛 TypeError：赋值路径（含闭包捕获 const 写 cell）统一拦截。
        if const_flag != 0 {
            let _ = self.emit_throw_error("TypeError", "Assignment to constant variable", ctx);
            return;
        }
        // 循环 update 段：被捕获绑定走寄存器而非 cell（C 风格 for 每迭代 fresh，
        // update 写寄存器供下一迭代 fresh 拷贝，不污染本迭代闭包捕获的 cell）。
        if ctx.register_update_names.iter().any(|n| n == name) {
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(reg),
                    Operand::Reg(val_reg),
                    Operand::Imm(const_flag),
                ));
                return;
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
            return;
        }
        // 目标若是被捕获 cell，走 CELL_SET
        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            ctx.inst(Inst::new(
                OpCode::CELL_SET,
                Operand::None,
                Operand::Reg(val_reg),
                Operand::Imm(cell_idx as u16),
            ));
            return;
        }
        let var_reg = ctx.lookup_or_global(name);
        ctx.inst(Inst::new(
            OpCode::STORE_VAR,
            Operand::Reg(var_reg),
            Operand::Reg(val_reg),
            Operand::Imm(const_flag),
        ));
    }

    /// with 内动态写入：对象有该属性则写对象，否则回退静态写入。
    ///
    /// # 边界与前提
    /// - 仅在 `with_stack` 非空且名字非 with 内部绑定时调用。
    /// - 对象属性存在性判定与动态读取一致（`in` 含原型链）。
    /// - 回退目标未在静态作用域声明时不登记全局（with 外不应可见）。
    pub(crate) fn emit_with_dynamic_write(&self, name: &str, val_reg: u32, const_flag: u16, ctx: &mut CompileCtx) {
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
            self.emit_identifier_store(name, val_reg, const_flag, ctx);
        }
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
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
