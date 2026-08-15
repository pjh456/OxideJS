//! 编译入口：`Compiler::compile` 串联 emit 与 lowering。

use oxide_bytecode::module::CompiledModule;
use oxide_emit::Emitter;
use oxide_ir::lower::lower;

/// 结构哈希 / 编译模块哈希 re-export（供编译缓存作键）。
pub use crate::hash::{compiled_module_hash, structural_hash};

/// 编译入口：状态 = DCE 开关 + RegAlloc 开关，编译其余状态在 emit/lower 内部。
pub struct Compiler {
    /// 是否在 emit 与 lower 之间运行死代码消除（默认开启）。
    enable_dce: bool,
    /// 是否在保守 DCE 后运行 liveness→精确 DCE→RegAlloc 链（默认开启）。
    /// 关闭时 vreg 原样当物理号走 lower 降级路径（>253 → RangeError）。
    enable_regalloc: bool,
}

impl Compiler {
    /// 构造 `Compiler`：默认开启 DCE 与 RegAlloc。
    pub fn new() -> Self {
        Self {
            enable_dce: true,
            enable_regalloc: true,
        }
    }

    /// 显式设置 DCE 开关（`with_dce(false)` 关闭死代码消除）。
    pub fn with_dce(self, enable: bool) -> Self {
        Self { enable_dce: enable, ..self }
    }

    /// 显式设置 RegAlloc 开关（`with_regalloc(false)` 关闭 liveness/精确 DCE/RegAlloc 链）。
    pub fn with_regalloc(self, enable: bool) -> Self {
        Self {
            enable_regalloc: enable,
            ..self
        }
    }

    /// 编译整个 script program：AST → IR（`Emitter::emit_program`）→ 统一 IR 管线。
    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile: starting...");
        let ir = Emitter::new().emit_program(program)?;
        self.compile_ir(ir)
    }

    /// 编译 ES module：AST → IR（`Emitter::emit_program_module`，含依赖模块链接）→ 统一 IR 管线。
    /// `module_path` 为模块文件的规范路径（依赖解析基准 = 其父目录），依赖加载经
    /// `loader` 解析。
    pub fn compile_module(
        &self, program: &oxide_parser::Program, module_path: &str,
        loader: &mut dyn oxide_emit::module::ModuleSourceLoader,
    ) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile_module: starting...");
        let ir = Emitter::new().emit_program_module(program, module_path, loader)?;
        let mut module = self.compile_ir(ir)?;
        module.is_es_module = true;
        Ok(module)
    }

    /// 统一 IR 后处理管线：（可选 DCE）→（可选 liveness/精确 DCE/RegAlloc）→
    /// bytecode（`lower`）→ 子模块拍平。
    fn compile_ir(&self, mut ir: oxide_ir::IRFunction) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile_ir: after emit {} insts", ir.insts.len());
        if self.enable_dce {
            oxide_dce::dce(&mut ir); // 顶层函数；nested 不递归
            crate::compiler_debug!("compile: after DCE {} insts", ir.insts.len());
        }
        if self.enable_regalloc && !ir.const_overflow {
            // 优化管线：liveness → 精确 DCE → 重算 liveness → RegAlloc
            // const_overflow 时跳过：程序已知无效（lower 报 "too many constants"），
            // 且超大常量池伴生的海量 vreg 会使 liveness 稠密 bitset 爆内存。
            let cfg = oxide_cfg::build_cfg(&ir);
            let live = oxide_liveness::liveness(&ir, &cfg);
            oxide_dce::dce_precise(&mut ir, &live); // 精确二轮消费第一轮 LiveInfo
            let cfg2 = oxide_cfg::build_cfg(&ir); // 精确 DCE 删指令后 inst 下标位移，CFG 重建
            let live2 = oxide_liveness::liveness(&ir, &cfg2); // 重算，绝不用过期 live
            oxide_regalloc::alloc(&mut ir, &live2)?; // 无可行染色 → RangeError 上抛
                                                     // RegAlloc 改写（vreg→phys + spill 插入）后补第三次 liveness：为调用点
                                                     // 存活上界提供物理级活集（改写过后的 LiveInfo 已过期，必须重算）。
            let cfg3 = oxide_cfg::build_cfg(&ir);
            let live3 = oxide_liveness::liveness(&ir, &cfg3);
            oxide_regalloc::encode_call_window(&mut ir, &live3);
            crate::compiler_debug!("compile: after regalloc {} insts", ir.insts.len());
        }
        crate::compiler_debug!("compile: done, {} instructions", ir.insts.len());
        let mut module = lower(&ir)?;
        crate::flatten::flatten_submodules(&mut module);
        Ok(module)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
