//! 子模块扁平化 pass：把嵌套的 `sub_modules` 树展开为全局唯一 id。
//!
//! `CREATE_CLOSURE` 在 emit 期用相对索引（当前模块 `nested` 的 1-based 位置）。
//! 逃逸闭包（函数对象在定义模块之外被调用）时相对索引不再可解。此 pass 在编译
//! 末端给整棵树分配 DFS 前序 `flat_id`（顶层 0），并把每条 `CREATE_CLOSURE` 的
//! imm16 重写为对应子模块的 `flat_id`。运行时以 `flat_id` 为平表下标，闭包自足，
//! 不再依赖调用上下文。

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode::{self, OpCode};

/// 给整棵子模块树分配扁平 id 并重写 `CREATE_CLOSURE` 操作数。
pub fn flatten_submodules(module: &mut CompiledModule) {
    let mut next_id = 1u32;
    assign_ids(module, &mut next_id);
}

fn assign_ids(module: &mut CompiledModule, next_id: &mut u32) {
    for sub in &mut module.sub_modules {
        sub.flat_id = *next_id;
        *next_id += 1;
    }
    for instr in &mut module.bytecode {
        if opcode::opcode(*instr) == OpCode::CREATE_CLOSURE {
            let rel = opcode::imm16(*instr) as usize;
            let flat = module.sub_modules.get(rel - 1).map(|s| s.flat_id).unwrap_or(0);
            let rd = opcode::rd(*instr);
            *instr = opcode::encode(
                OpCode::CREATE_CLOSURE,
                rd,
                (flat & 0xFF) as u8,
                ((flat >> 8) & 0xFF) as u8,
            );
        }
    }
    for sub in &mut module.sub_modules {
        assign_ids(sub, next_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_with_closure(nested: Vec<CompiledModule>) -> CompiledModule {
        let mut m = CompiledModule::new();
        m.sub_modules = nested;
        m
    }

    #[test]
    fn flatten_assigns_global_ids_and_rewrites_closures() {
        let mut leaf = CompiledModule::new();
        leaf.bytecode = vec![opcode::encode(OpCode::CREATE_CLOSURE, 1, 1, 0)];
        let mut mid = CompiledModule::new();
        mid.sub_modules = vec![leaf];
        mid.bytecode = vec![opcode::encode(OpCode::CREATE_CLOSURE, 2, 1, 0)];
        let mut top = module_with_closure(vec![mid]);

        flatten_submodules(&mut top);

        // 顶层 0，mid = 1，leaf = 2
        assert_eq!(top.flat_id, 0);
        assert_eq!(top.sub_modules[0].flat_id, 1);
        assert_eq!(top.sub_modules[0].sub_modules[0].flat_id, 2);
        // mid 的 CREATE_CLOSURE 相对 1 → flat 2
        assert_eq!(opcode::imm16(top.sub_modules[0].bytecode[0]), 2);
    }
}
