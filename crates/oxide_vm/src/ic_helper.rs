//! inline cache（IC）字节码流辅助函数。
//!
//! 汇聚全部 IC 读写/清零逻辑——属性分发与成员更新 handler 都经由此处，IC 格式
//! 变更（如未来的 side-table 迁移）只需改这一个文件。
//!
//! IC 项格式（操作码后 [`IC_EXT_WORDS`] 个扩展字 = [`IC_SLOTS`] 组二元组，槽 0 最前）：
//!   ext[i*2+0] = shape_id（低 24 位）| proto_depth（高 8 位，0 = 接收者自身属性）
//!   ext[i*2+1] = slot_index（32 位）
//!
//! 槽位约定：
//! - `shape_id == 0` 恒为空槽标记（合法 shape_id 从 [`EMPTY_SHAPE_ID`] = 1 起）。
//! - miss 写回 FIFO 滚动：新条目进槽 0、原槽 0..2 顺移到槽 1..3、槽 3 丢弃，
//!   空槽恒在尾部连续——命中遍历遇空槽即可 break。
//! - 命中路径零写：只读扩展字 + Cell 计数，不触发 bytecode COW。

use crate::vm_trace;
use oxide_bytecode::opcode::{self, Instr, OpCode, IC_EXT_WORDS, IC_SLOTS};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 读取 `pc` 处 IC 指令槽 0 的三元组并把 `pc` 推进越过全部扩展字（单态快路径用）。
/// 槽编码：[shape_id(24)|proto_depth(8)] [slot(32)]。
pub(crate) fn read_ic_slot0(bytecode: &[Instr], pc: &mut usize) -> (u32, u32, u8) {
    let ext0 = bytecode[*pc];
    let ext1 = bytecode[*pc + 1];
    *pc += IC_EXT_WORDS;
    (ext0 & 0x00FF_FFFF, ext1, (ext0 >> 24) as u8)
}

/// 把解析出的 `(shape_id, slot, proto_depth)` 以 FIFO 滚动写回 `pc - IC_EXT_WORDS`
/// 处的 IC 扩展字（位于当前指令之前）：新条目进槽 0，原槽 0..2 顺移到槽 1..3，
/// 最老的槽 3 丢弃。IC 未命中并完成属性解析后调用。
pub(crate) fn write_ic_back(bytecode: &mut [Instr], pc: usize, shape_id: u32, slot_index: u32, proto_depth: u8) {
    debug_assert!(pc >= IC_EXT_WORDS, "IC write-back requires {IC_EXT_WORDS} extension words before pc");
    vm_trace!("write_ic_back: pc={} shape_id={} slot={} depth={}", pc, shape_id, slot_index, proto_depth);
    let base = pc - IC_EXT_WORDS;
    // 读旧字后 FIFO 滚动：新条目写槽 0，原槽 i 顺移到槽 i+1，最老槽丢弃。
    let old = &bytecode[base..pc];
    let mut new_words = [0u32; IC_EXT_WORDS];
    new_words[0] = (shape_id & 0x00FF_FFFF) | ((proto_depth as u32) << 24);
    new_words[1] = slot_index;
    for i in 1..IC_SLOTS {
        new_words[i * 2..(i + 1) * 2].copy_from_slice(&old[(i - 1) * 2..i * 2]);
    }
    bytecode[base..pc].copy_from_slice(&new_words);
}

/// 计算 `pc` 处指令之后的扩展字个数（按 opcode 语义推进，逐指令字节序一致）。
///
/// 定长族返回固定字数；变长族依指令内容：spread 调用从首字读 nstatic|nspread、
/// TEMPLATE_STR 从首字读 segment_count、NEW_OBJECT 从 a 槽读属性数（键表）。
/// `clear_ic_caches` 依赖此函数逐指令定位，任何新增变长 ext opcode 必须在此登记。
fn ext_word_count(bytecode: &[Instr], pc: usize) -> usize {
    let op = opcode::opcode(bytecode[pc]);
    // IC 系固定 IC_EXT_WORDS 扩展字（多态槽组）。
    if op.has_ic_ext_words() {
        return IC_EXT_WORDS;
    }
    match op {
        OpCode::SPILL
        | OpCode::UNSPILL
        | OpCode::CALL
        | OpCode::CALL_NATIVE
        | OpCode::NEW_EXPRESSION
        | OpCode::SUPER_CALL
        | OpCode::DEFINE_ACCESSOR
        | OpCode::DEFINE_ACCESSOR_DYNAMIC
        | OpCode::DEFINE_PROP_ATTRS
        | OpCode::DELETE_PROP_STATIC
        | OpCode::REST_OBJECT
        | OpCode::INIT_PRIVATE => 1,
        OpCode::DEFINE_ACCESSOR_ATTRS | OpCode::GET_PRIVATE | OpCode::SET_PRIVATE | OpCode::PRIVATE_BRAND_IN => 2,
        OpCode::CALL_SPREAD | OpCode::NEW_EXPRESSION_SPREAD | OpCode::SUPER_CALL_SPREAD => {
            let header = bytecode.get(pc + 1).copied().unwrap_or(0);
            1 + (header & 0xFF) as usize + ((header >> 8) & 0xFF) as usize
        }
        OpCode::TEMPLATE_STR => {
            let header = bytecode.get(pc + 1).copied().unwrap_or(0);
            1 + ((header >> 16) & 0xFFFF) as usize
        }
        OpCode::NEW_OBJECT => opcode::a(bytecode[pc]) as usize,
        _ => 0,
    }
}

/// 把流中所有 IC 扩展字清零，使全部缓存 shape 失效。
///
/// 逐指令按 opcode 的 ext 字数推进，避开变长 ext opcode（CALL_SPREAD/TEMPLATE_STR/
/// NEW_OBJECT 键表）导致的字节错位——错位会把非 IC 指令误当 IC 扩展字清零。
pub(crate) fn clear_ic_caches(bytecode: &mut [Instr]) {
    let mut i = 0;
    while i < bytecode.len() {
        let op = opcode::opcode(bytecode[i]);
        if op.has_ic_ext_words() && i + IC_EXT_WORDS < bytecode.len() {
            for word in &mut bytecode[i + 1..=i + IC_EXT_WORDS] {
                *word = 0;
            }
        }
        i += 1 + ext_word_count(bytecode, i);
    }
}

/// 沿原型链上溯 `proto_depth` 步，返回目标对象的裸指针（链不足时返回 null）。
/// `proto_depth` 与 IC 写回语义一致：1 = 接收者的直接原型（`obj.proto()`）。
#[inline(always)]
fn resolve_proto_target_raw(obj: *const JsObject, proto_depth: u8) -> *const JsObject {
    if proto_depth == 0 {
        return obj;
    }
    let obj_ref = unsafe { &*obj };
    let mut cursor = obj_ref.proto();
    for _ in 1..proto_depth {
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

/// 多态遍历判定：调用方槽 0 单态判定失败后，顺序检查槽 1..3。
/// 空槽（shape_id==0）恒在尾部（FIFO 滚动保证），遇空即停。
#[inline(never)]
pub(crate) fn ic_get_hit_poly(obj: &JsObject, bytecode: &[Instr], ext_pc: usize) -> Option<JsValue> {
    for i in 1..IC_SLOTS {
        let base = ext_pc + i * 2;
        let shape_id = bytecode[base] & 0x00FF_FFFF;
        if shape_id == 0 {
            break;
        }
        let slot_index = bytecode[base + 1];
        let proto_depth = (bytecode[base] >> 24) as u8;
        if proto_depth == 0 {
            if obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
                let v = obj.get_prop_shape(slot_index);
                return Some(v);
            }
            continue;
        }
        let target = resolve_proto_target_raw(obj as *const JsObject, proto_depth);
        if target.is_null() {
            continue;
        }
        let target_ref = unsafe { &*target };
        if target_ref.shape_id() == shape_id && slot_index < target_ref.prop_vec_len() as u32 {
            return Some(target_ref.get_prop_shape(slot_index));
        }
    }
    None
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

/// 多态遍历判定：调用方槽 0 单态判定失败后，顺序检查槽 1..3。
/// 空槽（shape_id==0）恒在尾部（FIFO 滚动保证），遇空即停。
#[inline(never)]
pub(crate) fn ic_set_hit_poly(obj: &mut JsObject, bytecode: &[Instr], ext_pc: usize, value: JsValue) -> bool {
    for i in 1..IC_SLOTS {
        let base = ext_pc + i * 2;
        let shape_id = bytecode[base] & 0x00FF_FFFF;
        if shape_id == 0 {
            break;
        }
        let slot_index = bytecode[base + 1];
        let proto_depth = (bytecode[base] >> 24) as u8;
        if proto_depth == 0 {
            if obj.shape_id() == shape_id && slot_index < obj.prop_vec_len() as u32 {
                obj.set_prop_shape(slot_index, value);
                return true;
            }
            continue;
        }
        let target = resolve_proto_target_raw(obj as *const JsObject, proto_depth);
        if target.is_null() {
            continue;
        }
        let target_ref = unsafe { &*target };
        if target_ref.shape_id() == shape_id && slot_index < target_ref.prop_vec_len() as u32 {
            unsafe { &mut *(target as *mut JsObject) }.set_prop_shape(slot_index, value);
            return true;
        }
    }
    false
}
