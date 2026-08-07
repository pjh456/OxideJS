//! inline cache（IC）字节码流辅助函数。
//!
//! 汇聚全部 IC 读写/清零逻辑——属性分发与成员更新 handler 都经由此处，IC 格式
//! 变更（如未来的 side-table 迁移）只需改这一个文件。
//!
//! IC 项格式（操作码后 3 个扩展字）：
//!   ext0 = shape_id（24 位）
//!   ext1 = slot_index（32 位）
//!   ext2 = proto_depth（u8，0 = 接收者自身属性）

use crate::vm_trace;
use oxide_bytecode::opcode::{self, Instr};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 读取 `pc` 处 IC 指令后的三个扩展字，把 `pc` 前进越过它们，
/// 返回解码的 `(shape_id, slot, proto_depth)`。
pub(crate) fn read_ic_entry(bytecode: &[Instr], pc: &mut usize) -> (u32, u32, u8) {
    let ext0 = bytecode[*pc];
    let ext1 = bytecode[*pc + 1];
    let ext2 = bytecode[*pc + 2];
    *pc += 3;
    (ext0 & 0x00FF_FFFF, ext1, (ext2 & 0xFF) as u8)
}

/// 把解析出的 `(shape_id, slot, proto_depth)` 写回 `pc - 3` 处的三个 IC 扩展字
/// （位于当前指令之前）。IC 未命中并完成属性解析后调用。
pub(crate) fn write_ic_back(bytecode: &mut [Instr], pc: usize, shape_id: u32, slot_index: u32, proto_depth: u8) {
    debug_assert!(pc >= 3, "IC write-back requires 3 extension words before pc");
    vm_trace!("write_ic_back: pc={} shape_id={} slot={} depth={}", pc, shape_id, slot_index, proto_depth);
    bytecode[pc - 3] = shape_id & 0x00FF_FFFF;
    bytecode[pc - 2] = slot_index;
    bytecode[pc - 1] = proto_depth as u32;
}

/// 把流中所有 IC 扩展字清零，使全部缓存 shape 失效。
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

/// 沿原型链上溯 `proto_depth` 步，返回目标对象的裸指针（链不足时返回 null）。
#[inline(always)]
fn resolve_proto_target_raw(obj: *const JsObject, proto_depth: u8) -> *const JsObject {
    if proto_depth == 0 {
        return obj;
    }
    let obj_ref = unsafe { &*obj };
    let mut cursor = obj_ref.proto();
    for _ in 0..proto_depth {
        if !cursor.is_object() {
            return std::ptr::null();
        }
        let proto_ptr = cursor.as_js_object_ptr();
        if proto_ptr.is_null() {
            return std::ptr::null();
        }
        cursor = unsafe { (*proto_ptr).proto() };
    }
    if cursor.is_object() {
        let ptr = cursor.as_js_object_ptr();
        if !ptr.is_null() {
            return ptr as *const JsObject;
        }
    }
    std::ptr::null()
}

#[inline(always)]
pub(crate) fn ic_get_hit(obj: &JsObject, shape_id: u32, slot_index: u32, proto_depth: u8) -> Option<JsValue> {
    if shape_id == 0 {
        return None;
    }
    if proto_depth == 0 {
        if obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
            let v = obj.get_prop_shape(slot_index);
            eprintln!("[DBG] ic_get_hit shape={} slot={} apc={} v={:?}", shape_id, slot_index, obj.array_prop_count, v);
            return Some(v);
        }
        return None;
    }
    let target = resolve_proto_target_raw(obj as *const JsObject, proto_depth);
    if target.is_null() {
        return None;
    }
    let target_ref = unsafe { &*target };
    if target_ref.shape_id() == shape_id && slot_index < target_ref.prop_vec_len() as u32 {
        Some(target_ref.get_prop_shape(slot_index))
    } else {
        None
    }
}

#[inline(always)]
pub(crate) fn ic_set_hit(obj: &mut JsObject, shape_id: u32, slot_index: u32, proto_depth: u8, value: JsValue) -> bool {
    if shape_id == 0 {
        return false;
    }
    if proto_depth == 0 {
        if obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
            obj.set_prop_shape(slot_index, value);
            return true;
        }
        return false;
    }
    let target = resolve_proto_target_raw(obj as *const JsObject, proto_depth);
    if target.is_null() {
        return false;
    }
    let target_ref = unsafe { &*target };
    if target_ref.shape_id() == shape_id && slot_index < target_ref.prop_vec_len() as u32 {
        unsafe { &mut *(target as *mut JsObject) }.set_prop_shape(slot_index, value);
        true
    } else {
        false
    }
}

