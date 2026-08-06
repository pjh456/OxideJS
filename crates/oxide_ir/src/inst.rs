//! IR 指令结构 + 构造 API。
//!
//! `ext` 内部是裸 u32（lowering 直拼字节码扩展字）。扩展字的**数量与语义值编码**
//! 全部由 `inst_*` 构造 API 保证——emit 代码不直接触碰 `ext` 字段。

use oxide_bytecode::opcode::OpCode;
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

    // ── IC 系：ext = [0, 0, 0]（shape/slot/proto 占位字，VM 运行时 patch）──

    /// 内联缓存读属性：结果写入 `dst`，属性键为 `key`。
    pub fn ic_get(dst: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::IC_GET_PROP, Operand::None, dst, key, &[0, 0, 0])
    }

    /// 内联缓存写属性：`obj[key] = value`。
    pub fn ic_set(obj: Operand, value: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::IC_SET_PROP, obj, value, key, &[0, 0, 0])
    }

    /// 成员自增：`obj[key]++`，val 为当前值寄存器。
    pub fn member_inc(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::MEMBER_INC, obj, val, key, &[0, 0, 0])
    }

    /// 成员自减：`obj[key]--`，val 为当前值寄存器。
    pub fn member_dec(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::MEMBER_DEC, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值加法：`obj[key] += val`。
    pub fn compound_member_add(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_ADD, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值减法：`obj[key] -= val`。
    pub fn compound_member_sub(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_SUB, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值乘法：`obj[key] *= val`。
    pub fn compound_member_mul(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_MUL, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值除法：`obj[key] /= val`。
    pub fn compound_member_div(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_DIV, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值取模：`obj[key] %= val`。
    pub fn compound_member_mod(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_MOD, obj, val, key, &[0, 0, 0])
    }

    /// 成员复合赋值指数：`obj[key] **= val`。
    pub fn compound_member_exp(obj: Operand, val: Operand, key: Operand) -> Self {
        Self::with_ext(OpCode::COMPOUND_MEMBER_EXP, obj, val, key, &[0, 0, 0])
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

    // ── 其他带 ext 字 ──

    /// 定义访问器属性：home 为宿主对象，get/set 为访问器函数寄存器，key_idx 为属性名常量下标。
    pub fn define_accessor(home: Operand, get: Operand, set: Operand, key_idx: u32) -> Self {
        Self::with_ext(OpCode::DEFINE_ACCESSOR, home, get, set, &[key_idx])
    }

    /// 静态删除属性：obj 同时放 rd/a 槽，const_idx 为属性名常量下标。
    pub fn delete_prop_static(obj: Operand, const_idx: u32) -> Self {
        Self::with_ext(OpCode::DELETE_PROP_STATIC, obj, obj, Operand::None, &[const_idx])
    }

    /// 对象 rest 展开：`{...src, 排除 excluded_idx 常量列出的键}` 存入 `rest`。
    pub fn rest_object(rest: Operand, src: Operand, excluded_idx: u32) -> Self {
        Self::with_ext(OpCode::REST_OBJECT, rest, src, Operand::None, &[excluded_idx])
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

    // ── 无 ext：立即数/索引指令（拆字是 lowering 职责）──

    /// 加载常量池常量：`dst = constants[idx]`。a 槽 Const 下标由 lowering 拆字。
    pub fn load_const(dst: Operand, idx: u16) -> Self {
        Self::new(OpCode::LOAD_CONST, dst, Operand::Const(idx), Operand::None)
    }

    /// 创建闭包：`dst = nested[sub_idx]` 实例化。a 槽 Imm 子函数下标由 lowering 拆字。
    pub fn create_closure(dst: Operand, sub_idx: u16) -> Self {
        Self::new(OpCode::CREATE_CLOSURE, dst, Operand::Imm(sub_idx), Operand::None)
    }

    // ── 跳转族：label 放 b 槽，offset 计算是 lowering 职责 ──

    /// 无条件跳转。label 放 b 槽，offset 由 lowering 回填。
    pub fn jmp(label: LabelId) -> Self {
        Self::new(OpCode::JMP, Operand::None, Operand::None, Operand::Label(label))
    }

    /// 条件寄存器为 false 时跳转。
    pub fn jmp_if_false(cond_reg: u8, label: LabelId) -> Self {
        Self::new(
            OpCode::JMP_IF_FALSE,
            Operand::Reg(cond_reg as u32),
            Operand::None,
            Operand::Label(label),
        )
    }

    /// 条件寄存器为 true 时跳转。
    pub fn jmp_if_true(cond_reg: u8, label: LabelId) -> Self {
        Self::new(
            OpCode::JMP_IF_TRUE,
            Operand::Reg(cond_reg as u32),
            Operand::None,
            Operand::Label(label),
        )
    }

    /// 条件寄存器为 null/undefined 时跳转（`??` / 可选链短路）。
    pub fn jmp_if_nullish(cond_reg: u8, label: LabelId) -> Self {
        Self::new(
            OpCode::JMP_IF_NULLISH,
            Operand::Reg(cond_reg as u32),
            Operand::None,
            Operand::Label(label),
        )
    }

    /// try 块起始，label 指向对应的 catch/finally 处理入口。
    pub fn try_begin(label: LabelId) -> Self {
        Self::new(OpCode::TRY_BEGIN, Operand::None, Operand::None, Operand::Label(label))
    }

    /// try-finally 块起始，label 指向 finally 入口。
    pub fn try_finally_begin(label: LabelId) -> Self {
        Self::new(OpCode::TRY_FINALLY_BEGIN, Operand::None, Operand::None, Operand::Label(label))
    }

    // ── 寄存器契约（DCE 与 liveness 共用，D-06）──

    /// 本指令定义的寄存器（None 槽按 Reg(0) 映射；CALL 系含隐式 reg 0）。
    /// 无写入返回 None。
    pub fn def_reg(&self) -> Option<u32> {
        // TODO: Pattern 2 表逐 opcode 实现
        None
    }

    /// 本指令读取的寄存器（None 槽按 Reg(0) 映射；HALT 特判 reg 0；TEMPLATE_STR 解析 ext）。
    pub fn use_regs(&self) -> SmallVec<[u32; 4]> {
        // TODO: Pattern 2 表逐 opcode 实现
        SmallVec::new()
    }
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
    fn ic_instructions_carry_three_zero_ext_words() {
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
        ];
        for inst in &insts {
            assert_eq!(inst.ext.as_slice(), &[0, 0, 0], "IC op {} must carry 3 zero ext words", inst.op);
            assert!(inst.ext.len() == 3);
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

        let rest = Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7);
        assert_eq!(rest.ext.as_slice(), &[7]);
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

    // ── def_reg / use_regs 契约测试（Pattern 2 表，反直觉槽位手工 IR 锁定）──

    #[test]
    fn binary_op_def_rd_use_ab() {
        let inst = Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[1, 2]);
    }

    #[test]
    fn unary_op_def_rd_use_a() {
        let inst = Inst::new(OpCode::NEG, Operand::Reg(0), Operand::Reg(1), Operand::None);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[1]);
    }

    #[test]
    fn get_prop_def_is_a_slot_not_rd() {
        // Pitfall 3：GET_PROP rd=obj 是 use，结果写 a 槽
        let inst = Inst::new(OpCode::GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(1));
        assert_eq!(inst.use_regs().as_slice(), &[0, 2]);
    }

    #[test]
    fn get_prop_dynamic_def_is_b_slot() {
        // Pitfall 3：GET_PROP_DYNAMIC rd=obj 是 use，结果写 b 槽
        let inst = Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(2));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1]);
    }

    #[test]
    fn ic_get_prop_a_slot_is_use_and_def() {
        // Pitfall 3：IC_GET_PROP a 槽既是对象 use 又是结果 def
        let inst = Inst::ic_get(Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(1));
        let uses = inst.use_regs();
        assert_eq!(uses.as_slice(), &[1, 2]);
    }

    #[test]
    fn call_def_is_implicit_reg0_with_arg_range() {
        // Pitfall 1：CALL rd=callee 是 use，结果隐式写 reg 0；参数 b..b+nargs 连续
        let inst = Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn call_native_def_is_implicit_reg0() {
        let inst = Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 1);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2, 3]);
    }

    #[test]
    fn new_expression_uses_ctor_and_arg_range() {
        let inst = Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 2);
        assert_eq!(inst.def_reg(), Some(3));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn super_call_uses_arg_range() {
        let inst = Inst::super_call(Operand::Reg(3), Operand::Reg(1), 1);
        assert_eq!(inst.def_reg(), Some(3));
        assert_eq!(inst.use_regs().as_slice(), &[1]);
    }

    #[test]
    fn halt_implicitly_uses_reg0() {
        // Pitfall 2：HALT 返回 regs[0]，顶层 LOAD_VAR(None, r) 链靠它保活
        let inst = Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None);
        assert_eq!(inst.def_reg(), None);
        assert_eq!(inst.use_regs().as_slice(), &[0]);
    }

    #[test]
    fn void_defs_rd_no_use() {
        // VM 的 VOID handler 不读 a 槽
        let inst = Inst::new(OpCode::VOID, Operand::Reg(1), Operand::Reg(2), Operand::None);
        assert_eq!(inst.def_reg(), Some(1));
        assert!(inst.use_regs().is_empty());
    }

    #[test]
    fn load_const_create_closure_load_upvalue_no_use() {
        // a 槽是 Const/Imm 立即数，非寄存器
        let lc = Inst::load_const(Operand::Reg(1), 5);
        assert_eq!(lc.def_reg(), Some(1));
        assert!(lc.use_regs().is_empty());

        let cc = Inst::create_closure(Operand::Reg(1), 0);
        assert_eq!(cc.def_reg(), Some(1));
        assert!(cc.use_regs().is_empty());

        let lu = Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(1), Operand::Imm(0), Operand::None);
        assert_eq!(lu.def_reg(), Some(1));
        assert!(lu.use_regs().is_empty());
    }

    #[test]
    fn none_slot_maps_to_reg0() {
        // A3：None 槽与 lower.rs operand_to_u8 一致，统一映射物理 reg 0
        let lv = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::None, Operand::None);
        assert_eq!(lv.use_regs().as_slice(), &[0]);

        let cg = Inst::new(OpCode::CELL_GET, Operand::Reg(0), Operand::None, Operand::Imm(0));
        assert_eq!(cg.use_regs().as_slice(), &[0]);
    }

    #[test]
    fn template_str_parses_expr_regs_from_ext() {
        // ext[0] 跳过；后续 seg>>31==1 则低 8 位是 expr_reg（A1，emit 8 位编码）
        let with_expr = Inst::template_str(Operand::Reg(1), 2, 10, &[0x1234, 0x8000_0000 | 5]);
        assert_eq!(with_expr.use_regs().as_slice(), &[5]);

        // 纯 quasi（无表达式）：不误报寄存器
        let no_expr = Inst::template_str(Operand::Reg(1), 1, 10, &[0x1234]);
        assert!(no_expr.use_regs().is_empty());
    }

    #[test]
    fn jmp_if_false_uses_cond_reg() {
        let inst = Inst::jmp_if_false(3, 9);
        assert_eq!(inst.def_reg(), None);
        assert_eq!(inst.use_regs().as_slice(), &[3]);
    }

    #[test]
    fn jmp_and_try_no_use() {
        assert!(Inst::jmp(9).use_regs().is_empty());
        assert!(Inst::try_begin(9).use_regs().is_empty());
        assert!(Inst::try_finally_begin(9).use_regs().is_empty());
    }

    #[test]
    fn make_cell_no_def_uses_rd() {
        let inst = Inst::new(OpCode::MAKE_CELL, Operand::Reg(5), Operand::None, Operand::None);
        assert_eq!(inst.def_reg(), None);
        assert_eq!(inst.use_regs().as_slice(), &[5]);
    }

    #[test]
    fn cell_set_and_store_upvalue_use_a_no_def() {
        let cs = Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(1), Operand::Imm(0));
        assert_eq!(cs.def_reg(), None);
        assert_eq!(cs.use_regs().as_slice(), &[1]);

        let su = Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(1), Operand::Imm(0));
        assert_eq!(su.def_reg(), None);
        assert_eq!(su.use_regs().as_slice(), &[1]);
    }

    #[test]
    fn return_throw_use_rd_with_none_mapping_reg0() {
        let ret = Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None);
        assert_eq!(ret.def_reg(), None);
        assert_eq!(ret.use_regs().as_slice(), &[0]);

        let thr = Inst::new(OpCode::THROW, Operand::Reg(2), Operand::None, Operand::None);
        assert_eq!(thr.def_reg(), None);
        assert_eq!(thr.use_regs().as_slice(), &[2]);
    }

    #[test]
    fn rest_object_uses_a() {
        let inst = Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[1]);
    }

    #[test]
    fn for_in_next_defs_rd_no_use() {
        let inst = Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(0), Operand::None, Operand::None);
        assert_eq!(inst.def_reg(), Some(0));
        assert!(inst.use_regs().is_empty());
    }

    #[test]
    fn store_var_def_rd_use_a() {
        let inst = Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[1]);
    }
}
