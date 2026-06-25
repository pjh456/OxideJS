//! Inline-cache (IC) bytecode-stream helpers.
//!
//! These operate purely on the bytecode `Instr` slice — they read/clear/write
//! the IC extension words embedded after IC-bearing opcodes. They take no `Vm`
//! state, so they live outside `Vm` for clearer ownership and to ease the future
//! IC side-table migration.

use oxide_bytecode::opcode::{self, Instr};

/// Write a resolved `(shape_id, slot)` back into the three IC extension words
/// that precede the current `pc`. Called on an IC miss after resolving a
/// property, so the next execution of this site hits the cache.
pub(crate) fn write_ic_back(bytecode: &mut [Instr], pc: usize, shape_id: u32, slot_index: u32) {
    debug_assert!(pc >= 3, "IC write-back requires 3 extension words before pc");
    bytecode[pc - 3] = shape_id & 0x00FF_FFFF;
    bytecode[pc - 2] = slot_index;
    bytecode[pc - 1] = 0;
}

/// Zero every IC extension word in the stream, invalidating all cached shapes.
/// Used on `rerun()` so re-execution never derefs a stale cached pointer.
pub(crate) fn clear_ic_caches(bytecode: &mut [Instr]) {
    let mut i = 0;
    while i < bytecode.len() {
        let op = opcode::opcode(bytecode[i]);
        if op.has_ic_ext_words() {
            if i + 3 < bytecode.len() {
                bytecode[i + 1] = 0;
                bytecode[i + 2] = 0;
                bytecode[i + 3] = 0;
            }
            i += 4;
        } else {
            i += 1;
        }
    }
}
