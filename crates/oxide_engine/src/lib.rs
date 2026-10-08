//! oxide_engine：compile+run 便捷层（执行层与入口层之间）。
//!
//! `Engine` 持 `Arc<KernelCore>` + `Arc<VmPool>`，把入口 crate 各自内联装配的
//! 「parse → compile（经 CodeForge 缓存）→ run（经 VmPool 或独立 Vm）」流水线
//! 收口为单一实现源。
//!
//! 关键约定：
//! - 池恒建（`min/max_pool_size` 取自内核配置）；不借池的入口（每测试独立
//!   Vm）付一次可忽略的预热成本，不引入可选池分支。
//! - `compile` / `compile_with` 统一经 CodeForge 缓存；命中返回同一模块
//!   （结构哈希键，语义等价）。
//! - `new_vm` 是不经池的独立 Vm；动态编译服务与池内 Vm 同口径注入
//!   （缺省编译服务），调用方无需再注入。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_compiler::DefaultCompilerService;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use oxide_vm::vm_pool::{VmGuard, VmPool};
use oxide_vm::JsValue;

/// 引擎便捷类型：持共享内核与 VM 池，提供 compile / eval / spawn / new_vm 面。
///
/// 入口 crate（CLI、test262 runner）按进程或按 worker 各建一个，经其方法完成
/// parse → compile → run 流水线；同一引擎内全部 Vm 共享同一内核与池。
pub struct Engine {
    kernel: Arc<KernelCore>,
    pool: Arc<VmPool>,
}

impl Engine {
    /// 由内核配置建引擎：`KernelCore::new` 后按配置 `min/max_pool_size` 建池。
    ///
    /// # 边界与前提
    /// - `min_pool_size` 受 `max_pool_size` 钳制（池预热路径同口径）。
    ///
    /// # 副作用
    /// - 初始化日志系统（`KernelCore::new` 内完成，幂等）。
    /// - 同步预热 `min_pool_size` 个 Vm。
    pub fn new(config: KernelConfig) -> Self {
        let kernel = KernelCore::new(config);
        let pool = VmPool::new(
            Arc::clone(&kernel),
            Arc::new(DefaultCompilerService),
            kernel.config.min_pool_size,
            kernel.config.max_pool_size,
        );
        Self { kernel, pool }
    }

    /// parse + 缺省编译（`Compiler::new()`），经 CodeForge 缓存，返回模块。
    ///
    /// # 边界与前提
    /// - 解析失败返回各条消息换行拼接的单一错误串。
    ///
    /// # 副作用
    /// - 缓存未命中时编译产物写入 CodeForge。
    pub fn compile(&self, source: &str) -> Result<Arc<CompiledModule>, String> {
        self.compile_with(source, &Compiler::new())
    }

    /// parse + 自定义编译器编译，经 CodeForge 缓存，返回模块。
    ///
    /// 供 `no_dce` / `no_regalloc` / `repl_persist` 等旗标面：编译器由调用方
    /// 构造传入，无缺省臂，漏传即旗标失效。
    ///
    /// # 步骤
    /// 1. 局部分配器上解析源码为 Program。
    /// 2. 结构哈希作缓存键，CodeForge 未命中时调给定编译器编译并缓存。
    pub fn compile_with(&self, source: &str, compiler: &Compiler) -> Result<Arc<CompiledModule>, String> {
        // 局部分配器：AST 节点经 oxc bump 分配器分配，编译完成即弃。
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;

        let hash = compiled_module_hash(&program);
        self.kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program))
    }

    /// 编译 + 池借出 Vm 执行，返回完成值（eval 便捷路径）。
    pub fn eval(&self, source: &str) -> Result<JsValue, String> {
        let module = self.compile(source)?;
        let mut guard = self.pool.spawn();
        guard.vm_mut().run(&module)
    }

    /// 池借出 Vm（RAII `VmGuard`，drop 归还池）。
    pub fn spawn(&self) -> VmGuard {
        self.pool.spawn()
    }

    /// 建独立 Vm（不经池），供常驻 / 每测试独立场景。
    ///
    /// 动态编译服务与池内 Vm 同口径注入（缺省编译服务），调用方无需再注入。
    pub fn new_vm(&self) -> Vm {
        let mut vm = Vm::with_kernel_core(Arc::clone(&self.kernel));
        vm.set_compiler_service(Arc::new(DefaultCompilerService));
        vm
    }

    /// 只读访问共享内核。
    pub fn kernel(&self) -> &Arc<KernelCore> {
        &self.kernel
    }

    /// 只读访问 VM 池。
    pub fn pool(&self) -> &Arc<VmPool> {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::kernel::KernelConfig;

    fn test_engine() -> Engine {
        Engine::new(KernelConfig::minimal())
    }

    /// compile 把产物写入 CodeForge 缓存：同源码二次编译命中缓存，
    /// 返回同一模块且条目数不增。
    #[test]
    fn compile_caches_module_in_code_forge() {
        let engine = test_engine();
        let first = engine.compile("1 + 2").expect("编译应成功");
        assert_eq!(engine.kernel().code_forge().len(), 1, "首次编译应产生一个缓存条目");

        let second = engine.compile("1 + 2").expect("编译应成功");
        assert!(Arc::ptr_eq(&first, &second), "同源码二次编译应命中缓存返回同一模块");
        assert_eq!(engine.kernel().code_forge().len(), 1, "缓存命中不应新增条目");
    }

    /// eval 走编译 + 池借出 + 执行全路径，返回完成值。
    #[test]
    fn eval_returns_completion_value() {
        let engine = test_engine();
        let value = engine.eval("6 * 7.0").expect("执行应成功");
        assert!(value.is_number(), "完成值应为数字");
        assert_eq!(value.as_double(), 42.0, "完成值应为 42");
    }

    /// new_vm 是不经池的独立 Vm：能独立执行引擎的编译产物。
    #[test]
    fn new_vm_runs_independently() {
        let engine = test_engine();
        let module = engine.compile("10 + 5").expect("编译应成功");
        let mut vm = engine.new_vm();
        let value = vm.run(&module).expect("执行应成功");
        assert!(value.is_number(), "完成值应为数字");
        assert_eq!(value.as_int(), 15, "完成值应为 15");
    }

    /// 解析失败映射为单一错误串（各条消息换行拼接），且不污染缓存。
    #[test]
    fn parse_error_maps_to_joined_message() {
        let engine = test_engine();
        let err = match engine.compile("const x = {") {
            Ok(_) => panic!("解析失败应返回错误"),
            Err(e) => e,
        };
        assert!(!err.is_empty(), "解析错误消息不应为空");
        assert_eq!(engine.kernel().code_forge().len(), 0, "解析失败不应写入缓存");
    }
}
