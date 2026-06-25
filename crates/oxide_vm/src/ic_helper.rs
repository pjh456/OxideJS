//! Inline-cache (IC) bytecode-stream helpers.
//!
//! Contains ALL IC read/write/clear logic — property dispatch and member
//! update handlers route through here, so IC format changes (like the future
//! side-table migration) require editing only this file.

use oxide_bytecode::opcode::{self, Instr};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// Read the three IC extension words at `pc` (the bytes following an
/// IC-bearing opcode), advance `pc` past them, and return the decoded
/// `(shape_id, slot)`.
pub(crate) fn read_ic_entry(bytecode: &[Instr], pc: &mut usize) -> (u32, u32) {
    let ext0 = bytecode[*pc];
    let ext1 = bytecode[*pc + 1];
    *pc += 3;
    (ext0 & 0x00FF_FFFF, ext1)
}

/// Write a resolved `(shape_id, slot)` back into the three IC extension words
/// at `pc - 3` (they precede the current instruction). Called on an IC miss
/// after resolving a property, so the next execution of this site hits the cache.
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

#[inline(always)]
pub(crate) fn ic_get_hit(obj: &JsObject, shape_id: u32, slot_index: u32) -> Option<JsValue> {
    if shape_id != 0 && obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
        Some(obj.get_prop_at(slot_index))
    } else {
        None
    }
}

#[inline(always)]
pub(crate) fn ic_set_hit(obj: &mut JsObject, shape_id: u32, slot_index: u32, value: JsValue) -> bool {
    if shape_id != 0 && obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
        obj.set_prop_at(slot_index, value);
        true
    } else {
        false
    }
}
