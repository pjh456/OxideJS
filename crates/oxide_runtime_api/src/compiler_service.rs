//! 动态编译服务接口：`Vm` 经它把源码编译为 `CompiledModule`，
//! 解耦 `oxide_vm` 对 `oxide_compiler` / `oxide_parser` 的直接依赖。

use oxide_bytecode::module::CompiledModule;

/// 模块源码加载器：按 `base_dir` 与 `specifier` 定位依赖模块源码。
///
/// 供 `compile_module` 解析 ES module 依赖。返回 (源码, 规范路径)。
pub trait ModuleSourceLoader {
    /// 解析依赖模块：返回 (源码, 规范路径)。
    fn resolve(&mut self, base_dir: &str, specifier: &str) -> Result<(String, String), String>;
}

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

    /// 编译 ES module（含依赖加载），返回完整模块树。
    ///
    /// `module_path` 为模块文件的规范路径（依赖解析基准 = 其父目录），
    /// 依赖经 `loader` 解析。
    fn compile_module(
        &self, source: &str, module_path: &str, loader: &mut dyn ModuleSourceLoader,
    ) -> Result<CompiledModule, String>;
}
