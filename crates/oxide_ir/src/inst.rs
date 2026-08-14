//! IR 指令结构 + 构造 API。
//!
//! `ext` 内部是裸 u32（lowering 直拼字节码扩展字）。扩展字的**数量与语义值编码**
//! 全部由 `inst_*` 构造 API 保证——emit 代码不直接触碰 `ext` 字段。
//! 寄存器 def/use 契约与副作用判定在 `contract` 模块（`impl Inst`，DCE/liveness 共用）。

use oxide_bytecode::opcode::{OpCode, IC_EXT_WORDS};
use smallvec::SmallVec;

use crate::operand::{LabelId, Operand};

/// IR 指令：opcode + 三个操作数槽（rd/a/b）+ 扩展字 `ext`。
/// `ext` 内部是裸 u32，其数量与语义值编码由下方 `inst_*` 构造 API 保证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inst {
    pub op: OpCode,
    pub rd: Operand,
    pub a: Operand,
    pub b: Operand,
    pub ext: SmallVec<[u32; 4]>,
}

impl Inst {
    /// 基础构造：ext 为空。
    pub fn new(op: OpCode, rd: Operand, a: Operand, b: Operand) -> Self {
        Self {
            op,
            rd,
            a,
            b,
            ext: SmallVec::new(),
        }
    }

    fn with_ext(op: OpCode, rd: Operand, a: Operand, b: Operand, ext: &[u32]) -> Self {
        Self {
            op,
            rd,
            a,
            b,
            ext: SmallVec::from_slice(ext),
        }
    }

    // ── IC 系：ext = [0; IC_EXT_WORDS]（IC_SLOTS 组 shape/slot/proto 占位字，VM 运行时回填）──

    /// 内联缓存读属性：结果写入 `dst`，属性键为 `key`。
    pub fn ic_get(dst: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::IC_GET_PROP, Operand::None, dst, key, &[0; IC_EXT_WORDS])
    }

    /// 内联缓存写属性：`obj[key] = value`。
    pub fn ic_set(obj: Operand, value: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::IC_SET_PROP, obj, value, key, &[0; IC_EXT_WORDS])
    }

    /// 成员自增：`obj[key]++`，val 为当前值寄存器。
    pub fn member_inc(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::MEMBER_INC, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员自减：`obj[key]--`，val 为当前值寄存器。
    pub fn member_dec(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::MEMBER_DEC, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值加法：`obj[key] += val`。
    pub fn compound_member_add(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_ADD, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值减法：`obj[key] -= val`。
    pub fn compound_member_sub(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_SUB, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值乘法：`obj[key] *= val`。
    pub fn compound_member_mul(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_MUL, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值除法：`obj[key] /= val`。
    pub fn compound_member_div(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_DIV, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值取模：`obj[key] %= val`。
    pub fn compound_member_mod(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_MOD, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值指数：`obj[key] **= val`。
    pub fn compound_member_exp(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_EXP, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值按位与：`obj[key] &= val`。
    pub fn compound_member_bit_and(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_BIT_AND, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值按位或：`obj[key] |= val`。
    pub fn compound_member_bit_or(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_BIT_OR, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值按位异或：`obj[key] ^= val`。
    pub fn compound_member_bit_xor(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_BIT_XOR, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值左移：`obj[key] <<= val`。
    pub fn compound_member_shl(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_SHL, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值右移：`obj[key] >>= val`。
    pub fn compound_member_shr(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_SHR, obj, val, key, &[0; IC_EXT_WORDS])
    }

    /// 成员复合赋值无符号右移：`obj[key] >>>= val`。
    pub fn compound_member_ushr(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_USHR, obj, val, key, &[0; IC_EXT_WORDS])
    }

    // ── Call 系：ext = [nargs] ──

    /// 普通函数调用：rd=callee，a=this，b=首参，ext=\[nargs\]。参数从 `first_arg` 起连续占 nargs 个寄存器。
    pub fn call(callee: Operand, this: Operand, first_arg: Operand, nargs: u8) -> Self {
        Self::with_ext(OpCode::CALL, callee, this, first_arg, &[nargs as u32])
    }

    /// 原生函数调用（内置），不经 JS 调用协议。
    pub fn call_native(callee: Operand, this: Operand, first_arg: Operand, nargs: u8) -> Self {
        Self::with_ext(OpCode::CALL_NATIVE, callee, this, first_arg, &[nargs as u32])
    }

    /// `new` 表达式：结果写入 `result`，构造函数为 `constructor`。
    pub fn new_expression(result: Operand, constructor: Operand, first_arg: Operand, nargs: u8) -> Self {
        Self::with_ext(OpCode::NEW_EXPRESSION, result, constructor, first_arg, &[nargs as u32])
    }

    /// 派生类构造中的 `super(...)`：结果写入 `result`。
    pub fn super_call(result: Operand, first_arg: Operand, nargs: u8) -> Self {
        Self::with_ext(OpCode::SUPER_CALL, result, first_arg, Operand::None, &[nargs as u32])
    }

    /// spread 实参调用：rd=callee，a=this，b 槽未用。
    /// ext=[nstatic|(nspread<<8), 有序实参字…]；每个实参字：静态实参为寄存器号，
    /// spread 源为 `0x8000_0000 | 寄存器号`（高位标记），按源码求值序排列。
    pub fn call_spread(callee: Operand, this: Operand, words: &[u32]) -> Self {
        let ext = spread_ext(words);
        Self::with_ext(OpCode::CALL_SPREAD, callee, this, Operand::None, &ext)
    }

    /// spread 实参 `new`：结果写入 `result`，构造函数为 `constructor`。
    pub fn new_expression_spread(result: Operand, constructor: Operand, words: &[u32]) -> Self {
        let ext = spread_ext(words);
        Self::with_ext(OpCode::NEW_EXPRESSION_SPREAD, result, constructor, Operand::None, &ext)
    }

    /// 派生类构造中的 `super(...spread)`：结果写入 `result`。
    pub fn super_call_spread(result: Operand, words: &[u32]) -> Self {
        let ext = spread_ext(words);
        Self::with_ext(OpCode::SUPER_CALL_SPREAD, result, Operand::None, Operand::None, &ext)
    }

    // ── 其他带 ext 字 ──

    /// 私有成员读取：result=结果，obj=接收者，key=私有名 id。
    /// ext = [brand_reg, brand_id]：brand_reg 为当前类 brand 对象寄存器（0 表示跳过
    /// brand 检查，instance 字段路径），brand_id 为实例 brand 槽的私有名 id。
    pub fn get_private(result: Operand, obj: Operand, key: Operand, brand_reg: u32, brand_id: u32) -> Self {
        Self::with_ext(OpCode::GET_PRIVATE, result, obj, key, &[brand_reg, brand_id])
    }

    /// 私有名 `in` 判定：result=结果，obj=接收者，key=私有名 id。
    /// ext = [brand_reg, brand_id]：brand_reg 为当前类 brand 对象寄存器（0 表示跳过
    /// brand 检查），brand_id 为实例 brand 槽的私有名 id。判定 own 私有槽（字段）或
    /// own brand（方法/访问器，槽在 home）——不跨原型链（PrivateFieldIn）。
    pub fn private_brand_in(result: Operand, obj: Operand, key: Operand, brand_reg: u32, brand_id: u32) -> Self {
        Self::with_ext(OpCode::PRIVATE_BRAND_IN, result, obj, key, &[brand_reg, brand_id])
    }

    /// 私有成员写入：obj=接收者，value=新值，key=私有名 id。ext 语义同 `get_private`。
    pub fn set_private(obj: Operand, value: Operand, key: Operand, brand_reg: u32, brand_id: u32) -> Self {
        Self::with_ext(OpCode::SET_PRIVATE, obj, value, key, &[brand_reg, brand_id])
    }

    /// 私有成员初始化：target=接收者，value=初值，key=私有名 id。
    /// ext = [is_method]：1 表示私有方法槽（不可写），0 表示字段/brand 槽。
    pub fn init_private(target: Operand, value: Operand, key: Operand, is_method: bool) -> Self {
        Self::with_ext(OpCode::INIT_PRIVATE, target, value, key, &[is_method as u32])
    }

    /// 定义访问器属性：home 为宿主对象，get/set 为访问器函数寄存器，key_idx 为属性名常量下标。
    pub fn define_accessor(home: Operand, get: Operand, set: Operand, key_idx: u32) -> Self {
        Self::with_ext(OpCode::DEFINE_ACCESSOR, home, get, set, &[key_idx])
    }

    /// 定义访问器属性并指定描述符：ext = [key_idx, attrs]（attrs 位同 DEFINE_PROP_ATTRS）。
    pub fn define_accessor_attrs(home: Operand, get: Operand, set: Operand, key_idx: u32, attrs: u32) -> Self {
        Self::with_ext(OpCode::DEFINE_ACCESSOR_ATTRS, home, get, set, &[key_idx, attrs])
    }
    /// 定义访问器属性（运行时计算键）：home 为宿主对象，get/set 为访问器函数寄存器，
    /// key_reg 为键值寄存器（编码进 ext[0]，高位标记 `0x8000_0000 | key_reg`）。
    pub fn define_accessor_dynamic(home: Operand, get: Operand, set: Operand, key_reg: u32) -> Self {
        Self::with_ext(OpCode::DEFINE_ACCESSOR_DYNAMIC, home, get, set, &[0x8000_0000 | key_reg])
    }

    /// define 数据属性：target 为宿主对象，value/key 为寄存器。
    /// 不触发原型链 setter，与 SET_PROP 语义不同。
    pub fn define_prop(target: Operand, value: Operand, key: Operand) -> Self {
        Self::new(OpCode::DEFINE_PROP, target, value, key)
    }

    /// define 数据属性并指定描述符：ext = [attrs]（bit0=writable, bit1=enumerable, bit2=configurable，
    /// 与 PropAttributes 位一致）；class 方法/constructor/prototype 需要非枚举或不可写描述符。
    pub fn define_prop_attrs(target: Operand, value: Operand, key: Operand, attrs: u32) -> Self {
        Self::with_ext(OpCode::DEFINE_PROP_ATTRS, target, value, key, &[attrs])
    }
    /// 定义全局 var 绑定数据属性：target 为全局对象，value/key 为寄存器。
    /// 属性可写/可枚举/不可配置（脚本顶层 var/function 声明的属性描述符）。
    pub fn define_global_prop(target: Operand, value: Operand, key: Operand) -> Self {
        Self::new(OpCode::DEFINE_GLOBAL_PROP, target, value, key)
    }

    /// 静态删除属性：obj 同时放 rd/a 槽，const_idx 为属性名常量下标。
    pub fn delete_prop_static(obj: Operand, const_idx: u32) -> Self {
        Self::with_ext(OpCode::DELETE_PROP_STATIC, obj, obj, Operand::None, &[const_idx])
    }

    /// 对象 rest 展开：`{...src, 排除 excluded_idx 常量列出的键}` 存入 `rest`。
    /// `excl_arr` 为可选运行时 excluded 键数组寄存器（computed key 求值结果），
    /// None 时仅用编译期 excluded_idx 常量。
    pub fn rest_object(rest: Operand, src: Operand, excluded_idx: u32, excl_arr: Option<Operand>) -> Self {
        Self::with_ext(OpCode::REST_OBJECT, rest, src, excl_arr.unwrap_or(Operand::None), &[excluded_idx])
    }

    /// 对象字面量 spread 展开：把源 `src` 的可枚举自有属性写入目标对象 `rd`（原地改）。
    /// 语义 = CopyDataProperties 的"非 null/undefined 源"：null/undefined 合法（空展开），
    /// 与 REST_OBJECT（对 null/undefined 抛 TypeError）不同。
    pub fn spread_object(target: Operand, src: Operand) -> Self {
        Self::new(OpCode::SPREAD_OBJECT, target, src, Operand::None)
    }

    /// 对象字面量批量构造：`dst` 为新对象，a 槽编码纯静态数据键前缀的属性数（≤255），
    /// ext 为每键的常量池下标（`Constant::String`）。运行时按键序链式预建 shape，
    /// 后续逐个 `SET_PROP_BATCH` 纯槽写。
    pub fn new_object(dst: Operand, nprops: u32, key_idxs: &[u32]) -> Self {
        Self::with_ext(OpCode::NEW_OBJECT, dst, Operand::Imm(nprops as u16), Operand::None, key_idxs)
    }

    /// 对象字面量批量构造的纯槽写：`target[slot] = value`。slot 对应键序预建 shape 的槽位，
    /// 不做键解析/形状变更。
    pub fn set_prop_batch(target: Operand, value: Operand, slot: u16) -> Self {
        Self::new(OpCode::SET_PROP_BATCH, target, value, Operand::Imm(slot))
    }

    /// TEMPLATE_STR：变长 ext。首字打包 `(segment_count<<16) | total_len_hint`，
    /// 后续每 quasi 一项 `quasi_const_idx & 0x7FFF_FFFF`，其后若跟表达式再一项 `0x8000_0000 | expr_reg`。
    pub fn template_str(dst: Operand, segment_count: u32, total_len_hint: u16, parts: &[u32]) -> Self {
        let mut ext = SmallVec::with_capacity(2 + parts.len());
        ext.push(((segment_count & 0xFFFF) << 16) | (total_len_hint as u32 & 0xFFFF));
        ext.extend_from_slice(parts);
        Self {
            op: OpCode::TEMPLATE_STR,
            rd: dst,
            a: Operand::None,
            b: Operand::None,
            ext,
        }
    }

    // ── RegAlloc 辅助指令 ──

    /// 寄存器复制 rd = a（RegAlloc 区间拆分时搬值）。
    /// 不可用 LOAD_VAR/STORE_VAR 组合模拟——引入变量绑定语义会误触 const guard。
    pub fn inst_mov(dst: Operand, src: Operand) -> Self {
        Self::with_ext(OpCode::MOV, dst, src, Operand::None, &[])
    }

    /// 溢出：regs[rd] → spill_stack[frame_base + slot]，ext=[slot u16]。
    pub fn inst_spill(src: Operand, slot: u16) -> Self {
        Self::with_ext(OpCode::SPILL, src, Operand::None, Operand::None, &[slot as u32])
    }

    /// 恢复：spill_stack[frame_base + slot] → regs[rd]，ext=[slot u16]。
    pub fn inst_unspill(dst: Operand, slot: u16) -> Self {
        Self::with_ext(OpCode::UNSPILL, dst, Operand::None, Operand::None, &[slot as u32])
    }

    // ── 无 ext：立即数/索引指令（拆字是 lowering 职责）──

    /// 加载常量池常量：`dst = constants[idx]`。a 槽 Const 下标由 lowering 拆字。
    pub fn load_const(dst: Operand, idx: u16) -> Self {
        Self::new(OpCode::LOAD_CONST, dst, Operand::Const(idx), Operand::None)
    }

    /// 创建闭包：`dst = nested[sub_idx]` 实例化。a 槽 Imm 子函数下标由 lowering 拆字。
    pub fn create_closure(dst: Operand, sub_idx: u16) -> Self {
        Self::new(OpCode::CREATE_CLOSURE, dst, Operand::Imm(sub_idx), Operand::None)
    }

    /// 创建 arguments 对象：`dst = 当前帧的实参列表`。运行时从当前帧
    /// （CallFrame 或 inline 同步调用）的实参区构建，无寄存器 use。
    pub fn create_arguments(dst: Operand) -> Self {
        Self::new(OpCode::CREATE_ARGUMENTS, dst, Operand::None, Operand::None)
    }

    /// 创建 rest 参数数组：`dst = 当前帧实参区中下标 ≥ fixed_count 的实参组成的数组`。
    /// `fixed_count` 为 rest 之前的固定形参数，编码进 b 槽（Imm 单字节），运行时
    /// 与 CREATE_ARGUMENTS 同源读实参区，无寄存器 use。
    pub fn create_rest_array(dst: Operand, fixed_count: u32) -> Self {
        Self::new(OpCode::CREATE_REST_ARRAY, dst, Operand::None, Operand::Imm(fixed_count as u16))
    }

    /// 生成器让出：`rd` 为被让出的值；恢复时 `next(v)` 的 `v` 经 reg 0 交付
    /// （与 CALL 同协议，emit 用 `LOAD_VAR(None)` 读取）。
    pub fn yield_value(src: Operand) -> Self {
        Self::new(OpCode::YIELD, src, Operand::None, Operand::None)
    }

    /// `yield*` 委托：`rd` 为内层可迭代对象。运行时取迭代器并转发 next/return/throw，
    /// 委托完成值（内层 done 的 value）经 reg 0 交付，与 `yield_value` 同协议。
    pub fn yield_star(src: Operand) -> Self {
        Self::new(OpCode::YIELD_STAR, src, Operand::None, Operand::None)
    }

    /// 生成器 body 起点标记：调用时参数初始化完成后挂起于此，首次 `next()` 继续。
    pub fn suspend_body() -> Self {
        Self::new(OpCode::SUSPEND_BODY, Operand::None, Operand::None, Operand::None)
    }

    /// `await` 让出：`rd` 为被等待的值（运行时 PromiseResolve 包装）；异步帧挂起，
    /// promise settle 后恢复，值经 reg 0 交付（与 YIELD 同协议）。
    pub fn await_expr(src: Operand) -> Self {
        Self::new(OpCode::AWAIT, src, Operand::None, Operand::None)
    }

    // ── 跳转族：label 放 b 槽，offset 计算是 lowering 职责 ──

    /// 无条件跳转。label 放 b 槽，offset 由 lowering 回填。
    pub fn jmp(label: LabelId) -> Self {
        Self::new(OpCode::JMP, Operand::None, Operand::None, Operand::Label(label))
    }

    /// break 完成：label 指向循环/switch 出口，`crossed` 为逃出的 finally 域数
    /// （emit 词法计算，运行时据此逐个穿越 finally；0 表示未逃出，不产生完成语义）。
    /// 编码：rd 槽放 crossed（≤255），label 放 b 槽。
    pub fn brk(label: LabelId, crossed: u16) -> Self {
        Self::new(OpCode::BREAK, Operand::Imm(crossed), Operand::None, Operand::Label(label))
    }

    /// continue 完成：同 break，label 指向循环继续目标。
    pub fn cont(label: LabelId, crossed: u16) -> Self {
        Self::new(OpCode::CONTINUE, Operand::Imm(crossed), Operand::None, Operand::Label(label))
    }

    /// 条件寄存器为 false 时跳转。
    pub fn jmp_if_false(cond_reg: u32, label: LabelId) -> Self {
        Self::new(OpCode::JMP_IF_FALSE, Operand::Reg(cond_reg), Operand::None, Operand::Label(label))
    }

    /// 条件寄存器为 true 时跳转。
    pub fn jmp_if_true(cond_reg: u32, label: LabelId) -> Self {
        Self::new(OpCode::JMP_IF_TRUE, Operand::Reg(cond_reg), Operand::None, Operand::Label(label))
    }

    /// 条件寄存器为 null/undefined 时跳转（`??` / 可选链短路）。
    pub fn jmp_if_nullish(cond_reg: u32, label: LabelId) -> Self {
        Self::new(OpCode::JMP_IF_NULLISH, Operand::Reg(cond_reg), Operand::None, Operand::Label(label))
    }

    /// try 块起始，label 指向对应的 catch/finally 处理入口。
    pub fn try_begin(label: LabelId) -> Self {
        Self::new(OpCode::TRY_BEGIN, Operand::None, Operand::None, Operand::Label(label))
    }

    /// try-finally 块起始，label 指向 finally 入口。
    pub fn try_finally_begin(label: LabelId) -> Self {
        Self::new(OpCode::TRY_FINALLY_BEGIN, Operand::None, Operand::None, Operand::Label(label))
    }

    /// finally 体入口标记：运行时置位对应 try handler 的 `finally_active`。
    pub fn try_finally_enter() -> Self {
        Self::new(OpCode::TRY_FINALLY_ENTER, Operand::None, Operand::None, Operand::None)
    }
}

/// spread 调用系 ext 构造：首字打包 `nstatic | (nspread << 8)`，后续按源码求值序排列
/// 每个实参字（静态实参 = 寄存器号，spread 源 = `0x8000_0000 | 寄存器号`）。
fn spread_ext(words: &[u32]) -> SmallVec<[u32; 4]> {
    let nspread = words.iter().filter(|w| *w >> 31 == 1).count() as u32;
    let nstatic = (words.len() as u32) - nspread;
    let mut ext = SmallVec::with_capacity(1 + words.len());
    ext.push(nstatic | (nspread << 8));
    ext.extend_from_slice(words);
    ext
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inst_new_has_empty_ext() {
        let inst = Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert!(inst.ext.is_empty());
        assert_eq!(inst.op, OpCode::ADD);
    }

    #[test]
    fn ic_instructions_carry_ic_slots_zero_ext_words() {
        let insts = [
            Inst::ic_get(Operand::Reg(1), Operand::Reg(2)),
            Inst::ic_set(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::member_inc(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::member_dec(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_add(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_sub(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_mul(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_div(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_mod(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_exp(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_bit_and(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_bit_or(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_bit_xor(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_shl(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_shr(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
            Inst::compound_member_ushr(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        ];
        for inst in &insts {
            assert_eq!(
                inst.ext.as_slice(),
                &[0; IC_EXT_WORDS],
                "IC op {} must carry {IC_EXT_WORDS} zero ext words",
                inst.op
            );
            assert_eq!(inst.ext.len(), IC_EXT_WORDS);
        }
    }

    #[test]
    fn call_instructions_carry_nargs() {
        let call = Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3);
        assert_eq!(call.ext.as_slice(), &[3]);
        assert_eq!(call.rd, Operand::Reg(0));
        assert_eq!(call.a, Operand::Reg(1));
        assert_eq!(call.b, Operand::Reg(2));

        let native = Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0);
        assert_eq!(native.ext.as_slice(), &[0]);

        let new_expr = Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 2);
        assert_eq!(new_expr.ext.as_slice(), &[2]);
        assert_eq!(new_expr.rd, Operand::Reg(3));
        assert_eq!(new_expr.a, Operand::Reg(0));

        let super_call = Inst::super_call(Operand::Reg(3), Operand::Reg(1), 1);
        assert_eq!(super_call.ext.as_slice(), &[1]);
        assert_eq!(super_call.rd, Operand::Reg(3));
        assert_eq!(super_call.a, Operand::Reg(1));
    }

    #[test]
    fn single_ext_word_instructions() {
        let accessor = Inst::define_accessor(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 42);
        assert_eq!(accessor.ext.as_slice(), &[42]);
        assert_eq!(accessor.rd, Operand::Reg(0));
        assert_eq!(accessor.a, Operand::Reg(1));
        assert_eq!(accessor.b, Operand::Reg(2));

        let dyn_accessor = Inst::define_accessor_dynamic(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 7);
        assert_eq!(dyn_accessor.op, OpCode::DEFINE_ACCESSOR_DYNAMIC);
        assert_eq!(dyn_accessor.ext.as_slice(), &[0x8000_0000 | 7]);
        assert_eq!(dyn_accessor.rd, Operand::Reg(0));
        assert_eq!(dyn_accessor.a, Operand::Reg(1));
        assert_eq!(dyn_accessor.b, Operand::Reg(2));

        let rest = Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7, None);
        assert_eq!(rest.ext.as_slice(), &[7]);

        let spread = Inst::spread_object(Operand::Reg(0), Operand::Reg(1));
        assert_eq!(spread.op, OpCode::SPREAD_OBJECT);
        assert_eq!(spread.rd, Operand::Reg(0));
        assert_eq!(spread.a, Operand::Reg(1));
        assert_eq!(spread.b, Operand::None);
        assert!(spread.ext.is_empty());
    }

    #[test]
    fn spread_call_constructors_carry_header_and_source_regs() {
        let call = Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[10, 0x8000_0000 | 300]);
        assert_eq!(call.op, OpCode::CALL_SPREAD);
        assert_eq!(call.rd, Operand::Reg(0));
        assert_eq!(call.a, Operand::Reg(1));
        assert_eq!(call.b, Operand::None);
        assert_eq!(call.ext.as_slice(), &[1 | (1 << 8), 10, 0x8000_0000 | 300]);

        let ne = Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[0x8000_0000 | 7]);
        assert_eq!(ne.op, OpCode::NEW_EXPRESSION_SPREAD);
        assert_eq!(ne.rd, Operand::Reg(3));
        assert_eq!(ne.a, Operand::Reg(0));
        assert_eq!(ne.ext.as_slice(), &[1 << 8, 0x8000_0000 | 7]);

        let sc = Inst::super_call_spread(Operand::Reg(3), &[1, 0x8000_0000 | 5]);
        assert_eq!(sc.op, OpCode::SUPER_CALL_SPREAD);
        assert_eq!(sc.rd, Operand::Reg(3));
        assert_eq!(sc.a, Operand::None);
        assert_eq!(sc.b, Operand::None);
        assert_eq!(sc.ext.as_slice(), &[1 | (1 << 8), 1, 0x8000_0000 | 5]);
    }

    #[test]
    fn regalloc_constructors_carry_expected_slots() {
        let mov = Inst::inst_mov(Operand::Reg(3), Operand::Reg(7));
        assert_eq!(mov.op, OpCode::MOV);
        assert_eq!(mov.rd, Operand::Reg(3));
        assert_eq!(mov.a, Operand::Reg(7));
        assert_eq!(mov.b, Operand::None);
        assert!(mov.ext.is_empty());

        let spill = Inst::inst_spill(Operand::Reg(5), 42);
        assert_eq!(spill.op, OpCode::SPILL);
        assert_eq!(spill.rd, Operand::Reg(5));
        assert_eq!(spill.a, Operand::None);
        assert_eq!(spill.b, Operand::None);
        assert_eq!(spill.ext.as_slice(), &[42]);

        let unspill = Inst::inst_unspill(Operand::Reg(9), 0xFFFF);
        assert_eq!(unspill.op, OpCode::UNSPILL);
        assert_eq!(unspill.rd, Operand::Reg(9));
        assert_eq!(unspill.a, Operand::None);
        assert_eq!(unspill.b, Operand::None);
        assert_eq!(unspill.ext.as_slice(), &[0xFFFF]);
    }

    #[test]
    fn load_const_and_create_closure_keep_semantic_operands() {
        let lc = Inst::load_const(Operand::Reg(4), 300);
        assert_eq!(lc.a, Operand::Const(300));
        assert_eq!(lc.b, Operand::None);
        assert!(lc.ext.is_empty());

        let cc = Inst::create_closure(Operand::Reg(4), 5);
        assert_eq!(cc.a, Operand::Imm(5));
        assert_eq!(cc.b, Operand::None);
        assert!(cc.ext.is_empty());

        let ca = Inst::create_arguments(Operand::Reg(6));
        assert_eq!(ca.op, OpCode::CREATE_ARGUMENTS);
        assert_eq!(ca.rd, Operand::Reg(6));
        assert_eq!(ca.a, Operand::None);
        assert_eq!(ca.b, Operand::None);
        assert!(ca.ext.is_empty());
    }

    #[test]
    fn jump_family_puts_label_in_b_slot() {
        let jmp = Inst::jmp(9);
        assert_eq!(jmp.b, Operand::Label(9));
        assert_eq!(jmp.rd, Operand::None);

        let cond = Inst::jmp_if_false(3, 9);
        assert_eq!(cond.rd, Operand::Reg(3));
        assert_eq!(cond.b, Operand::Label(9));

        let true_jmp = Inst::jmp_if_true(3, 9);
        assert_eq!(true_jmp.b, Operand::Label(9));

        let nullish = Inst::jmp_if_nullish(3, 9);
        assert_eq!(nullish.b, Operand::Label(9));

        let try_begin = Inst::try_begin(9);
        assert_eq!(try_begin.b, Operand::Label(9));
        assert_eq!(try_begin.rd, Operand::None);

        let try_fin = Inst::try_finally_begin(9);
        assert_eq!(try_fin.b, Operand::Label(9));
    }

    #[test]
    fn template_str_packs_segment_count_and_hint() {
        let inst = Inst::template_str(Operand::Reg(1), 3, 10, &[0x1234, 0x8000_0000 | 5]);
        assert_eq!(inst.op, OpCode::TEMPLATE_STR);
        assert_eq!(inst.rd, Operand::Reg(1));
        assert_eq!(inst.ext.len(), 3);
        assert_eq!(inst.ext[0], (3 << 16) | 10);
        assert_eq!(inst.ext[1], 0x1234);
        assert_eq!(inst.ext[2], 0x8000_0000 | 5);
    }
}
