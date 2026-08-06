//! 编译入口：`Compiler::compile` 串联 emit 与 lowering。

use oxide_bytecode::module::CompiledModule;
use oxide_emit::Emitter;
use oxide_ir::lower::lower;

/// 结构哈希 / 编译模块哈希 re-export（供编译缓存作键）。
pub use crate::hash::{compiled_module_hash, structural_hash};

/// 编译入口：状态 = DCE 开关 + RegAlloc 开关，编译其余状态在 emit/lower 内部。
pub struct Compiler {
    /// 是否在 emit 与 lower 之间运行死代码消除（D-10：默认开启）。
    enable_dce: bool,
    /// 是否在保守 DCE 后运行 liveness→精确 DCE→RegAlloc 链（D-18：默认开启）。
    /// off 时 vreg 原样当物理号走 lower 降级路径（>253 → RangeError）。
    enable_regalloc: bool,
}

impl Compiler {
    /// 构造 `Compiler`：默认开启 DCE 与 RegAlloc（D-10/D-18）。
    pub fn new() -> Self {
        Self { enable_dce: true, enable_regalloc: true }
    }

    /// 显式设置 DCE 开关（`with_dce(false)` 关闭死代码消除）。
    pub fn with_dce(self, enable: bool) -> Self {
        Self { enable_dce: enable, ..self }
    }

    /// 显式设置 RegAlloc 开关（`with_regalloc(false)` 关闭 liveness/精确 DCE/RegAlloc 链）。
    pub fn with_regalloc(self, enable: bool) -> Self {
        Self { enable_regalloc: enable, ..self }
    }

    /// 编译整个 program：AST → IR（`Emitter::emit_program`）→（可选 DCE）→（可选
    /// liveness/精确 DCE/RegAlloc）→ bytecode（`lower`）。
    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile: starting...");
        let mut ir = Emitter::new().emit_program(program)?;
        if self.enable_dce {
            oxide_dce::dce(&mut ir); // 顶层函数；nested 不递归（D-03）
        }
        if self.enable_regalloc && !ir.const_overflow {
            // D-17 管线：liveness → 精确 DCE → 重算 liveness → RegAlloc
            // const_overflow 时跳过：程序已知无效（lower 报 "too many constants"），
            // 且超大常量池伴生的海量 vreg 会使 liveness 稠密 bitset 爆内存。
            let cfg = oxide_cfg::build_cfg(&ir);
            let live = oxide_liveness::liveness(&ir, &cfg);
            oxide_dce::dce_precise(&mut ir, &live); // 精确二轮消费第一轮 LiveInfo
            let cfg2 = oxide_cfg::build_cfg(&ir); // 精确 DCE 删指令后 inst 下标位移，CFG 重建
            let live2 = oxide_liveness::liveness(&ir, &cfg2); // 重算，绝不用过期 live
            oxide_regalloc::alloc(&mut ir, &live2)?; // D-02：无可行染色 → RangeError 上抛
        }
        crate::compiler_debug!("compile: done, {} instructions", ir.insts.len());
        lower(&ir)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
