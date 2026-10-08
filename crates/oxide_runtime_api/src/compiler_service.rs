//! 动态编译服务接口：`Vm` 经它把源码编译为 `CompiledModule`，
//! 解耦 `oxide_vm` 对 `oxide_compiler` / `oxide_parser` 的直接依赖。

use oxide_bytecode::module::CompiledModule;

/// 动态编译服务：`Vm` 经它把源码编译为 `CompiledModule`。
///
/// 无状态实现 `Send + Sync`，`Arc<dyn CompilerService>` 可跨线程共享
/// （worker 调度、多 realm 场景）。trait 对象安全，`Vm` 以
/// `Arc<dyn CompilerService>` 持服务句柄。
pub trait CompilerService: Send + Sync {
    /// 编译普通脚本（source_encoded 形态），返回完整模块树
    /// （根 flat_id=0 + 子模块）。
    ///
    /// 供 `Function` 构造器（wrap 源码）与 `$262.evalScript`（普通脚本）。
    fn compile_script(&self, source: &str) -> Result<CompiledModule, String>;

    /// 编译 eval 脚本（source_encoded + eval_script 形态），返回完整模块树。
    ///
    /// 顶层 var/function 声明落全局属性 configurable:true。
    fn compile_eval_script(&self, source: &str) -> Result<CompiledModule, String>;
}
