//! 错误构造/传播四守卫：运行时抛错指令序列与 TDZ / const 写检查。
//!
//! 均为 `Emitter` 方法，与各语法域 emit 族共用 `CompileCtx` 编译上下文。

use crate::compile_ctx::CompileCtx;
use crate::Emitter;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

impl Emitter {
    /// 生成运行时抛 `kind` 类型错误的指令序列，返回一个未定义 dummy 寄存器
    /// 保证 THROW 后不可达控制流的寄存器良定义。
    ///
    /// # 步骤
    /// 1. 取全局错误构造器并 LOAD。
    /// 2. 加载错误消息常量，`new {kind}(msg)` 构造错误对象。
    /// 3. THROW 抛出；尾接 dummy 值保持后续读引用有确定寄存器。
    ///
    /// # 边界与前提
    /// - `kind` 必须是已注册的全局构造器名（如 "ReferenceError"/"TypeError"）。
    pub(crate) fn emit_throw_error(&self, kind: &str, msg: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 构造器读 A 侧全局对象属性（冷路径，与已知 builtin 名读路由同形）：
        // delete 构造器后缺失抛 ReferenceError，与 V8 同形。仅留登记副作用。
        let _ = ctx.lookup_or_builtin(kind)?;
        let ctor = ctx.alloc_reg();
        let key_idx = ctx.add_constant(Constant::String(kind.to_string()));
        ctx.inst(Inst::new(OpCode::LOAD_GLOBAL, Operand::Reg(ctor), Operand::Const(key_idx), Operand::None));
        let msg_reg = ctx.alloc_reg();
        let msg_idx = ctx.add_constant(Constant::String(msg.to_string()));
        ctx.inst(Inst::load_const(Operand::Reg(msg_reg), msg_idx));
        let exc_reg = ctx.alloc_reg();
        ctx.inst(Inst::new_expression(Operand::Reg(exc_reg), Operand::Reg(ctor), Operand::Reg(msg_reg), 1));
        ctx.inst(Inst::new(OpCode::THROW, Operand::Reg(exc_reg), Operand::None, Operand::None));
        let dummy = ctx.alloc_reg();
        let undef_idx = ctx.add_constant(Constant::Undefined);
        ctx.inst(Inst::load_const(Operand::Reg(dummy), undef_idx));
        Ok(dummy)
    }

    /// 生成运行时抛 ReferenceError 的指令序列（TDZ 访问专用，语义见 [`emit_throw_error`]）。
    pub(crate) fn emit_tdz_throw(&self, msg: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        self.emit_throw_error("ReferenceError", msg, ctx)
    }

    /// 赋值目标 TDZ 检查：未初始化绑定在赋值引用解析时抛 ReferenceError。
    /// 须在 RHS 求值之前调用（规范：赋值 LHS 的 ResolveBinding 先于 RHS 副作用）。
    ///
    /// # 边界与前提
    /// - 被捕获名（upvalue）跳过：其 TDZ 由运行时 cell 初始化标志判定，编译期
    ///   快照对声明序晚于函数编译的绑定是陈旧的。
    ///
    /// # 副作用
    /// - TDZ 命中时发射 THROW 指令序列，其后指令不可达但保持寄存器良定义。
    pub(crate) fn emit_identifier_tdz_guard(&self, name: &str, ctx: &mut CompileCtx) -> Result<(), String> {
        // 被捕获绑定经运行时 cell 的初始化标志统一判定（cell 读写与 upvalue
        // 读写同面）：嵌套函数 ctx 继承的父作用域 initialized 快照是陈旧的
        // （函数声明编译早于 let/const 声明的初始化点），静态 throw 会误报，
        // 命中捕获名即跳过，真 TDZ 由运行时抛。
        if ctx.current_upvalue_captures.iter().any(|u| u.name == name) {
            return Ok(());
        }
        if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
            if !binding.initialized {
                let _ = self.emit_tdz_throw(&format!("Cannot access '{name}' before initialization"), ctx)?;
            }
        }
        Ok(())
    }

    /// const 写检查：已初始化的 const 绑定再赋值编译期抛 TypeError（与槽值无关）。
    /// 简单赋值在 RHS 求值之后、复合/更新在读旧值之前调用；解构赋值在写目标时调用。
    ///
    /// # 副作用
    /// - const 命中时发射 THROW 指令序列，其后写指令不可达但保持寄存器良定义。
    pub(crate) fn emit_const_write_guard(&self, name: &str, ctx: &mut CompileCtx) -> Result<(), String> {
        if ctx.lookup_const_flag(name) {
            let _ = self.emit_throw_error("TypeError", "Assignment to constant variable", ctx)?;
        }
        Ok(())
    }
}
