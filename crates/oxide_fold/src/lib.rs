//! 常量折叠 pass：单向前扫常量传播加就地改写（IR 层）。
//!
//! `fold` 对每个函数（递归 nested，每个 nested 函数有独立常量池与指令流，独立前扫）
//! 运行单向前扫：维护 per-register known 表记录各虚拟寄存器的已知常量值，命中
//! 改写表即就地改写——A 组一元与算术常量折叠改写为 `LOAD_CONST`（新常量入池），
//! B 组逻辑短路改写为 `MOV`（无新常量）。
//!
//! 不变量：
//! - 不删指令、不重映射 `label_pos`：改写只改变指令的 op 与操作数，不改变跳转
//!   结构，CFG 的块划分与边集不变，由下游 DCE 在折叠后的指令流上重建 CFG 吸收。
//! - 汇合点健全性：`label_pos` 指向的指令下标（控制流汇合点）整体清空 known 表，
//!   跨汇合的陈旧知识不用于折叠。
//! - 负零守卫：算术折叠做实际运算（f64 加），不取恒等捷径——`-0.0 + 0.0` 得
//!   `+0.0`（`Object.is` 可区分）。
//! - 池溢出守卫：常量池满（`u16::MAX`）时 A 组改写禁用（不新增常量），B 组
//!   （MOV）不受影响。
//!
//! 折值计算复用 `oxide_runtime_api::coercion` 的纯函数族（`to_number` /
//! `to_int32` / `to_boolean`），与 emit 层字面量折叠同源，杜绝两层折叠各自镜像
//! 运行时语义的漂移风险。

mod fold_log;
mod fold_table;
mod propagate;

use oxide_ir::IRFunction;

/// 常量传播与折叠：对函数（递归 nested）单向前扫，就地改写。
///
/// 返回是否发生改写（仅调试日志用）。接在 emit 与 DCE 之间：改写后失去消费者
/// 的 `LOAD_CONST` 与死 `MOV` 由下游 DCE 清理，本 pass 不删指令。
pub fn fold(f: &mut IRFunction) -> bool {
    let mut changed = propagate::fold_function(f);
    for nested in f.nested.iter_mut() {
        if fold(nested) {
            changed = true;
        }
    }
    if changed {
        fold_info!("fold: rewrites applied");
    }
    changed
}
