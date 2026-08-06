//! DCE 改写 pass：IRFunction → 死代码消除后 compact 重建。
//!
//! 改写 pass（D-09）：`&mut IRFunction` 就地 compact（D-06 方案 A 索引关联保证无借用冲突），
//! 三 Pass——Pass A 块级可达性（消费 `oxide_cfg::build_cfg`，Exception 边保守，D-07/Pitfall 4）、
//! Pass B 全函数 use 计数迭代删除死指令到不动点（D-01/D-02 连锁语义）、
//! Pass C mark-sweep 一次性重建 insts + label_pos 重映射（D-13，不用就地逐删平移）。
//! 本 pass 不碰 nested（D-03 不递归）、常量池（D-14 不清理）、n_registers（D-04 不收缩）、
//! label_count（D-13 不读不改）。中间产物（keep/use_count）用局部 Vec 显式传参，
//! 不做 struct 状态持有（无共享可变状态约定）。零 unsafe。

use oxide_ir::IRFunction;

/// 死代码消除：块级不可达删除 + 全函数 use 计数迭代删除到不动点 + mark-sweep 重建。
pub fn dce(f: &mut IRFunction) {
    // 空 IRFunction 退化形态：无指令可删（照 oxide_cfg empty_function 先例）。
    // 非空路径为 Task 2/3 骨架占位：Pass A 可达性 + Pass B use 计数迭代 + Pass C mark-sweep。
    if !f.insts.is_empty() {
        // TODO(Task 2/3)：Pass A/B/C 实现（当前骨架 no-op，等价于不改写）。
    }
}
