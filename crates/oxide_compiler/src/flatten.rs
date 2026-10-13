//! 子模块扁平化 pass：把嵌套的 `sub_modules` 树展开为全局唯一 id。
//!
//! `CREATE_CLOSURE` 在 emit 期用相对索引（当前模块 `nested` 的 1-based 位置）。
//! 逃逸闭包（函数对象在定义模块之外被调用）时相对索引不再可解。此 pass 在编译
//! 末端给整棵树分配 DFS 前序 `flat_id`（顶层 0），并把每条 `CREATE_CLOSURE` 的
//! imm16 重写为对应子模块的 `flat_id`。运行时以 `flat_id` 为平表下标，闭包自足，
//! 不再依赖调用上下文。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode::{self, OpCode};

/// 给整棵子模块树分配扁平 id 并重写 `CREATE_CLOSURE` 操作数。
///
/// TLA 模块（`is_async`）额外做 entry rehome：平表下标 0 是 native 哨兵
/// （`call_bytecode_function_inline` 拒绝 `sub_module_index == 0`），故在 `sub_modules`
/// 头部插一个占位槽（flat_id 0）、entry 落 flat_id 1、其余子模块顺延。
pub fn flatten_submodules(module: &mut CompiledModule) {
    let rehome = module.is_async;
    if rehome {
        module.sub_modules.insert(0, Arc::new(CompiledModule::new()));
        module.flat_id = 1;
        // entry 的 CREATE_CLOSURE 相对下标整体 +1（占位槽占据下标 1）。
        for instr in Arc::make_mut(&mut module.bytecode).iter_mut() {
            if opcode::opcode(*instr) == OpCode::CREATE_CLOSURE {
                let rel = opcode::imm16(*instr) as usize;
                let rd = opcode::rd(*instr);
                *instr = opcode::encode(
                    OpCode::CREATE_CLOSURE,
                    rd,
                    ((rel + 1) & 0xFF) as u8,
                    (((rel + 1) >> 8) & 0xFF) as u8,
                );
            }
        }
    }
    let mut next_id = if rehome { 2u32 } else { 1u32 };
    assign_ids(module, &mut next_id, rehome);
}

fn assign_ids(module: &mut CompiledModule, next_id: &mut u32, skip_placeholder: bool) {
    for (i, sub) in module.sub_modules.iter_mut().enumerate() {
        // TLA 模块首个子模块是占位槽（flat_id 0），不参与递增分配。
        if skip_placeholder && i == 0 {
            Arc::make_mut(sub).flat_id = 0;
            continue;
        }
        // 子模块以 Arc 共享；flatten 在编译末端跑（refcount 1），make_mut 无拷贝。
        Arc::make_mut(sub).flat_id = *next_id;
        *next_id += 1;
    }
    for instr in Arc::make_mut(&mut module.bytecode).iter_mut() {
        if opcode::opcode(*instr) == OpCode::CREATE_CLOSURE {
            let rel = opcode::imm16(*instr) as usize;
            // rel 应为 1-based 子模块下标；0 表示无引用（异常输入），按未解析处理。
            let flat = if rel > 0 {
                module.sub_modules.get(rel - 1).map(|s| s.flat_id).unwrap_or(0)
            } else {
                0
            };
            let rd = opcode::rd(*instr);
            *instr = opcode::encode(OpCode::CREATE_CLOSURE, rd, (flat & 0xFF) as u8, ((flat >> 8) & 0xFF) as u8);
        }
    }
    for sub in &mut module.sub_modules {
        assign_ids(Arc::make_mut(sub), next_id, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_with_closure(nested: Vec<Arc<CompiledModule>>) -> CompiledModule {
        let mut m = CompiledModule::new();
        m.sub_modules = nested;
        m
    }

    #[test]
    fn flatten_assigns_global_ids_and_rewrites_closures() {
        let mut leaf = CompiledModule::new();
        leaf.bytecode = Arc::from(vec![opcode::encode(OpCode::CREATE_CLOSURE, 1, 1, 0)]);
        let mut mid = CompiledModule::new();
        mid.sub_modules = vec![Arc::new(leaf)];
        mid.bytecode = Arc::from(vec![opcode::encode(OpCode::CREATE_CLOSURE, 2, 1, 0)]);
        let mut top = module_with_closure(vec![Arc::new(mid)]);

        flatten_submodules(&mut top);

        // 顶层 0，mid = 1，leaf = 2
        assert_eq!(top.flat_id, 0);
        assert_eq!(top.sub_modules[0].flat_id, 1);
        assert_eq!(top.sub_modules[0].sub_modules[0].flat_id, 2);
        // mid 的 CREATE_CLOSURE 相对 1 → flat 2
        assert_eq!(opcode::imm16(top.sub_modules[0].bytecode[0]), 2);
    }

    #[test]
    fn flatten_rehomes_async_entry_to_index_one() {
        // 非 TLA 模块：entry 落 flat_id 0，占位槽不存在。
        let mut sync = CompiledModule::new();
        sync.bytecode = Arc::from(vec![opcode::encode(OpCode::CREATE_CLOSURE, 1, 1, 0)]);
        sync.sub_modules = vec![Arc::new(CompiledModule::new())];
        flatten_submodules(&mut sync);
        assert_eq!(sync.flat_id, 0);
        assert_eq!(sync.sub_modules.len(), 1);
        assert_eq!(sync.sub_modules[0].flat_id, 1);

        // TLA 模块：占位槽 0、entry 落 1、子模块顺延 2。
        let mut tla = CompiledModule::new();
        tla.is_async = true;
        tla.bytecode = Arc::from(vec![opcode::encode(OpCode::CREATE_CLOSURE, 1, 1, 0)]);
        tla.sub_modules = vec![Arc::new(CompiledModule::new())];
        flatten_submodules(&mut tla);
        assert_eq!(tla.flat_id, 1);
        assert_eq!(tla.sub_modules.len(), 2);
        assert_eq!(tla.sub_modules[0].flat_id, 0); // 占位槽
        assert_eq!(tla.sub_modules[1].flat_id, 2);
        // entry 的 CREATE_CLOSURE 相对 1 → 占位槽插入后指向 flat 2
        assert_eq!(opcode::imm16(tla.bytecode[0]), 2);
    }
}
