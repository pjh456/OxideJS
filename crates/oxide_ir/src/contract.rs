//! Inst 寄存器契约：def/use 扫描 + 副作用判定（DCE 与 liveness 共用单源）。
//!
//! `def_reg` / `use_regs` / `is_pure` 是 `Inst` 的类型契约方法（`impl Inst` 与 `inst.rs`
//! 中的构造 API 同属一类型），落在 oxide_ir 契约层供 DCE 消费、liveness 复用。
//! 寄存器规则与 VM dispatch handler 行为一致：
//! CALL 隐式写 reg 0、GET_PROP 结果写 a/b 槽、HALT 隐式读 reg 0、None 槽映射 Reg(0)。

use oxide_bytecode::opcode::OpCode;
use smallvec::SmallVec;

use crate::inst::Inst;
use crate::operand::Operand;
use crate::IRFunction;

impl Inst {
    /// 本指令定义的寄存器（None 槽按 Reg(0) 映射；CALL 系含隐式 reg 0）。
    /// 无写入返回 None。
    pub fn def_reg(&self) -> Option<u32> {
        match self.op {
            // CALL 系：结果隐式写 reg 0，rd 是 callee 的 use
            OpCode::CALL | OpCode::CALL_NATIVE | OpCode::CALL_SPREAD => Some(0),
            // GET_PROP 系：结果写 a/b 槽而非 rd
            OpCode::GET_PROP | OpCode::IC_GET_PROP => reg_of(&self.a),
            OpCode::GET_PROP_DYNAMIC => reg_of(&self.b),
            // 成员读写：val 槽原地更新（a 槽 / b 槽）
            OpCode::MEMBER_INC | OpCode::MEMBER_DEC => reg_of(&self.a),
            OpCode::DYN_MEMBER_INC | OpCode::DYN_MEMBER_DEC => reg_of(&self.b),
            OpCode::COMPOUND_MEMBER_ADD
            | OpCode::COMPOUND_MEMBER_SUB
            | OpCode::COMPOUND_MEMBER_MUL
            | OpCode::COMPOUND_MEMBER_DIV
            | OpCode::COMPOUND_MEMBER_MOD
            | OpCode::COMPOUND_MEMBER_EXP
            | OpCode::COMPOUND_MEMBER_BIT_AND
            | OpCode::COMPOUND_MEMBER_BIT_OR
            | OpCode::COMPOUND_MEMBER_BIT_XOR
            | OpCode::COMPOUND_MEMBER_SHL
            | OpCode::COMPOUND_MEMBER_SHR
            | OpCode::COMPOUND_MEMBER_USHR => reg_of(&self.a),
            // 无 def：控制流 / 写共享状态 / 纯写对象
            OpCode::HALT
            | OpCode::RETURN
            | OpCode::THROW
            | OpCode::JMP
            | OpCode::BREAK
            | OpCode::CONTINUE
            | OpCode::JMP_IF_FALSE
            | OpCode::JMP_IF_TRUE
            | OpCode::JMP_IF_NULLISH
            | OpCode::TRY_BEGIN
            | OpCode::TRY_END
            | OpCode::TRY_FINALLY_BEGIN
            | OpCode::TRY_FINALLY_END
            | OpCode::MAKE_CELL
            | OpCode::CELL_SET
            | OpCode::STORE_UPVALUE
            | OpCode::SET_PROP
            | OpCode::SET_PROP_DYNAMIC
            | OpCode::IC_SET_PROP
            | OpCode::SET_ELEM
            | OpCode::SET_PRIVATE
            | OpCode::INIT_PRIVATE
            | OpCode::DELETE_PROP_STATIC
            | OpCode::DELETE_PROP_DYNAMIC
            | OpCode::DEFINE_ACCESSOR
            | OpCode::DEFINE_PROP
            | OpCode::SET_HOME_OBJECT
            // SPILL 写 VM spill 栈而非寄存器（恢复由 UNSPILL 写 rd）
            | OpCode::SPILL
            | OpCode::FOR_IN_INIT
            | OpCode::FOR_OF_INIT
            | OpCode::FOR_IN_CLEANUP
            | OpCode::FOR_OF_CLOSE => None,
            // 其余指令 rd 即 def（算术/比较/位/逻辑/加载族/迭代器 NEXT 等）
            _ => reg_of(&self.rd),
        }
    }

    /// 本指令读取的寄存器（None 槽按 Reg(0) 映射；HALT 特判 reg 0；TEMPLATE_STR 解析 ext）。
    pub fn use_regs(&self) -> SmallVec<[u32; 4]> {
        let mut uses = SmallVec::new();
        match self.op {
            // CALL 系：rd=callee, a=this, b..b+nargs 参数区间
            OpCode::CALL | OpCode::CALL_NATIVE => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
                let nargs = self.ext.first().copied().unwrap_or(0);
                push_range(&mut uses, reg_of(&self.b), nargs);
            }
            // NEW_EXPRESSION：a=ctor, b..b+nargs
            OpCode::NEW_EXPRESSION => {
                push_operand(&mut uses, &self.a);
                let nargs = self.ext.first().copied().unwrap_or(0);
                push_range(&mut uses, reg_of(&self.b), nargs);
            }
            // SUPER_CALL：a..a+nargs
            OpCode::SUPER_CALL => {
                let nargs = self.ext.first().copied().unwrap_or(0);
                push_range(&mut uses, reg_of(&self.a), nargs);
            }
            // spread 调用系：ext 内嵌有序实参字（静态寄存器号 / `0x8000_0000|reg` spread 源）
            OpCode::CALL_SPREAD => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
                for &w in self.ext.iter().skip(1) {
                    uses.push(w & 0x7FFF_FFFF);
                }
            }
            OpCode::NEW_EXPRESSION_SPREAD => {
                push_operand(&mut uses, &self.a);
                for &w in self.ext.iter().skip(1) {
                    uses.push(w & 0x7FFF_FFFF);
                }
            }
            OpCode::SUPER_CALL_SPREAD => {
                for &w in self.ext.iter().skip(1) {
                    uses.push(w & 0x7FFF_FFFF);
                }
            }
            // GET_PROP 系：结果写 a/b 槽，rd=obj 是 use
            OpCode::GET_PROP => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.b);
            }
            OpCode::GET_PROP_DYNAMIC => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
            }
            OpCode::IC_GET_PROP => {
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // 成员更新：obj + key 是 use，val 槽是 def
            OpCode::MEMBER_INC | OpCode::MEMBER_DEC => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.b);
            }
            OpCode::DYN_MEMBER_INC | OpCode::DYN_MEMBER_DEC => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
            }
            OpCode::COMPOUND_MEMBER_ADD
            | OpCode::COMPOUND_MEMBER_SUB
            | OpCode::COMPOUND_MEMBER_MUL
            | OpCode::COMPOUND_MEMBER_DIV
            | OpCode::COMPOUND_MEMBER_MOD
            | OpCode::COMPOUND_MEMBER_EXP
            | OpCode::COMPOUND_MEMBER_BIT_AND
            | OpCode::COMPOUND_MEMBER_BIT_OR
            | OpCode::COMPOUND_MEMBER_BIT_XOR
            | OpCode::COMPOUND_MEMBER_SHL
            | OpCode::COMPOUND_MEMBER_SHR
            | OpCode::COMPOUND_MEMBER_USHR => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // 复合赋值：rd 与 a 都是 use（coerce 读双寄存器）
            OpCode::COMPOUND_ADD
            | OpCode::COMPOUND_SUB
            | OpCode::COMPOUND_MUL
            | OpCode::COMPOUND_DIV
            | OpCode::COMPOUND_MOD
            | OpCode::COMPOUND_EXP
            | OpCode::COMPOUND_AND
            | OpCode::COMPOUND_OR
            | OpCode::COMPOUND_XOR
            | OpCode::COMPOUND_SHL
            | OpCode::COMPOUND_SHR
            | OpCode::COMPOUND_USHR => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
            }
            // 自增/自减：读 rd（写 rd 和 a）
            OpCode::INC_PRE | OpCode::INC_POST | OpCode::DEC_PRE | OpCode::DEC_POST => {
                push_operand(&mut uses, &self.rd);
            }
            // 二元运算：a, b
            OpCode::ADD
            | OpCode::SUB
            | OpCode::MUL
            | OpCode::DIV
            | OpCode::MOD
            | OpCode::EQ
            | OpCode::NEQ
            | OpCode::LT
            | OpCode::GT
            | OpCode::LTE
            | OpCode::GTE
            | OpCode::AND
            | OpCode::OR
            | OpCode::NULLISH
            | OpCode::STRICT_EQ
            | OpCode::STRICT_NEQ
            | OpCode::BIT_AND
            | OpCode::BIT_OR
            | OpCode::BIT_XOR
            | OpCode::SHL
            | OpCode::SHR
            | OpCode::USHR
            | OpCode::INSTANCEOF
            | OpCode::IN
            | OpCode::CREATE_REGEXP => {
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // 一元：a（emit 常 rd==a）
            OpCode::NEG | OpCode::NOT | OpCode::UNARY_PLUS | OpCode::BIT_NOT | OpCode::TYPEOF => {
                push_operand(&mut uses, &self.a);
            }
            // 无条件跳转 / try 标记：b 是 Label，无寄存器
            OpCode::JMP
            | OpCode::BREAK
            | OpCode::CONTINUE
            | OpCode::TRY_BEGIN
            | OpCode::TRY_FINALLY_BEGIN
            | OpCode::TRY_END
            | OpCode::TRY_FINALLY_END
            | OpCode::FOR_IN_CLEANUP
            | OpCode::FOR_OF_CLOSE => {}
            // 条件跳转：rd=cond
            OpCode::JMP_IF_FALSE | OpCode::JMP_IF_TRUE | OpCode::JMP_IF_NULLISH => {
                push_operand(&mut uses, &self.rd);
            }
            // HALT：隐式读 reg 0（顶层返回值）
            OpCode::HALT => uses.push(0),
            // RETURN/THROW：读 rd（None→0）
            OpCode::RETURN | OpCode::THROW => push_operand(&mut uses, &self.rd),
            // 加载族：a 槽是 Const/Imm 立即数，无寄存器 use
            OpCode::LOAD_CONST
            | OpCode::CREATE_CLOSURE
            | OpCode::LOAD_UPVALUE
            | OpCode::VOID
            | OpCode::NEW_OBJECT
            | OpCode::NEW_ARRAY
            | OpCode::CREATE_ARGUMENTS
            | OpCode::NOP => {}
            // 变量读写：LOAD_VAR/STORE_VAR/CELL_GET 读 a（None→0）
            OpCode::LOAD_VAR | OpCode::STORE_VAR | OpCode::CELL_GET => {
                push_operand(&mut uses, &self.a);
            }
            // RegAlloc 辅助：MOV 读 src（a 槽）；SPILL 读待溢出寄存器（rd）；UNSPILL 仅写 rd 无寄存器读
            OpCode::MOV => push_operand(&mut uses, &self.a),
            OpCode::SPILL => push_operand(&mut uses, &self.rd),
            OpCode::UNSPILL => {}
            // MAKE_CELL：cell 初值读 rd；CELL_SET/STORE_UPVALUE 读 a
            OpCode::MAKE_CELL => push_operand(&mut uses, &self.rd),
            OpCode::CELL_SET | OpCode::STORE_UPVALUE => push_operand(&mut uses, &self.a),
            // 写对象属性：rd/a/b 全 use
            OpCode::SET_PROP
            | OpCode::SET_PROP_DYNAMIC
            | OpCode::IC_SET_PROP
            | OpCode::SET_ELEM
            | OpCode::INIT_PRIVATE
            | OpCode::DEFINE_ACCESSOR
            | OpCode::DEFINE_PROP => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // 私有写：rd=对象、a=值、b=键是 use，额外读 ext[0]（brand 对象寄存器）
            OpCode::SET_PRIVATE => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
                let brand_reg = self.ext.first().copied().unwrap_or(0);
                if brand_reg != 0 {
                    uses.push(brand_reg);
                }
            }
            // 私有读/品牌检查：a=对象、b=私有键是 use，rd 是结果 def
            // GET_PRIVATE 额外读 ext[0]（brand 对象寄存器，0 表示跳过检查）
            OpCode::GET_PRIVATE => {
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
                let brand_reg = self.ext.first().copied().unwrap_or(0);
                if brand_reg != 0 {
                    uses.push(brand_reg);
                }
            }
            OpCode::PRIVATE_BRAND_IN => {
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // delete：读 obj（+dynamic 读 key）
            OpCode::DELETE_PROP_STATIC => push_operand(&mut uses, &self.rd),
            OpCode::DELETE_PROP_DYNAMIC => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.b);
            }
            // super 属性读：a=this, b=key
            OpCode::SUPER_GET_PROP | OpCode::SUPER_STATIC_GET_PROP => {
                push_operand(&mut uses, &self.a);
                push_operand(&mut uses, &self.b);
            }
            // SET_HOME_OBJECT：rd=func, a=home
            OpCode::SET_HOME_OBJECT => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
            }
            // for-in/of 迭代器：INIT 读 a；NEXT/DONE 读迭代器栈隐式状态
            OpCode::FOR_IN_INIT | OpCode::FOR_OF_INIT => push_operand(&mut uses, &self.a),
            OpCode::FOR_IN_NEXT | OpCode::FOR_IN_DONE | OpCode::FOR_OF_NEXT | OpCode::FOR_OF_DONE => {}
            // REST_OBJECT：读 a（ext 是 excluded_idx 常量）
            OpCode::REST_OBJECT => push_operand(&mut uses, &self.a),
            // SPREAD_OBJECT：原地改目标对象，读目标（rd）与源（a）
            OpCode::SPREAD_OBJECT => {
                push_operand(&mut uses, &self.rd);
                push_operand(&mut uses, &self.a);
            }
            // TEMPLATE_STR：解析 ext，跳过 ext[0]，后续 seg>>31==1 则低 8 位是 expr_reg
            OpCode::TEMPLATE_STR => {
                for seg in self.ext.iter().skip(1) {
                    if seg >> 31 == 1 {
                        uses.push(seg & 0xFF);
                    }
                }
            }
            // 占位 opcode（emit 不产）：按无 use 保守处理，不影响合法产物
            OpCode::SWITCH_TABLE
            | OpCode::PROFILE_SHAPE
            | OpCode::PROFILE_BRANCH
            | OpCode::PROFILE_CALL
            | OpCode::FORK
            | OpCode::JOIN => {}
            // TO_OBJECT：rd 原地转换（读且写）
            OpCode::TO_OBJECT => {
                push_operand(&mut uses, &self.rd);
            }
        }
        uses
    }

    /// 本指令是否无观察副作用（LOAD_VAR 的 This+derived 特判需要函数元信息）。
    ///
    /// 纯 = 结果未用时可删。表外 opcode 一律视为有副作用，宁少删不错删。
    pub fn is_pure(&self, f: &IRFunction) -> bool {
        match self.op {
            // 算术 / 比较 / 位 / 逻辑：coerce 对象路径有抛错风险，仍按纯处理（运行时等价兜底）
            OpCode::ADD
            | OpCode::SUB
            | OpCode::MUL
            | OpCode::DIV
            | OpCode::MOD
            | OpCode::NEG
            | OpCode::UNARY_PLUS
            | OpCode::EQ
            | OpCode::NEQ
            | OpCode::LT
            | OpCode::GT
            | OpCode::LTE
            | OpCode::GTE
            | OpCode::STRICT_EQ
            | OpCode::STRICT_NEQ
            | OpCode::BIT_AND
            | OpCode::BIT_OR
            | OpCode::BIT_XOR
            | OpCode::SHL
            | OpCode::SHR
            | OpCode::USHR
            | OpCode::BIT_NOT
            | OpCode::AND
            | OpCode::OR
            | OpCode::NOT
            | OpCode::NULLISH => true,
            // 分配 / 常量 / 空操作（a 槽非寄存器）
            OpCode::NOP
            | OpCode::LOAD_CONST
            | OpCode::VOID
            | OpCode::TYPEOF
            | OpCode::NEW_OBJECT
            | OpCode::NEW_ARRAY => true,
            // MOV：寄存器复制无副作用，可删无害
            OpCode::MOV => true,
            // SPILL/UNSPILL：RegAlloc 插入的指令，删 SPILL 后 UNSPILL 读陈旧值，永不删
            OpCode::SPILL | OpCode::UNSPILL => false,
            // 闭包 / cell 读取：无抛错路径
            OpCode::CREATE_CLOSURE | OpCode::LOAD_UPVALUE | OpCode::CELL_GET => true,
            // 模板字符串：纯拼接写 rd，无抛错（use 统计覆盖其 ext）
            OpCode::TEMPLATE_STR => true,
            // LOAD_VAR：仅 a==This 且 derived 构造函数读 this 时可能抛 ReferenceError
            OpCode::LOAD_VAR => !(self.a == Operand::This && f.is_derived_constructor),
            // STORE_VAR：写变量目标寄存器。脚本顶层/模块作用域的变量是全局可观察状态，
            // 删除会改变外部观察结果；函数内局部未用赋值的优化交给精确 DCE。
            OpCode::STORE_VAR => false,
            // 其余全部有副作用（getter/调用/控制流/迭代器/复合更新/写共享状态/占位防御）
            _ => false,
        }
    }
}

/// 操作数 → 物理寄存器号。None 槽映射 reg 0（与 lower.rs operand_to_u8 一致）；
/// Const/Imm/Label 不是寄存器槽，返回 None。
fn reg_of(o: &Operand) -> Option<u32> {
    match o {
        Operand::Reg(r) => Some(*r),
        Operand::This => Some(254),
        Operand::NewTarget => Some(255),
        Operand::None => Some(0),
        Operand::Const(_) | Operand::Imm(_) | Operand::Label(_) => None,
    }
}

/// 把寄存器操作数推入 use 集合（非寄存器槽跳过）。
fn push_operand(uses: &mut SmallVec<[u32; 4]>, o: &Operand) {
    if let Some(r) = reg_of(o) {
        uses.push(r);
    }
}

/// 推入连续参数区间 [first, first+nargs)。
fn push_range(uses: &mut SmallVec<[u32; 4]>, first: Option<u32>, nargs: u32) {
    if let Some(f) = first {
        uses.extend((0..nargs).map(|i| f + i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inst::Inst;
    use crate::operand::Operand;
    use crate::IRFunction;

    // ── def_reg / use_regs 契约测试（反直觉槽位断言）──

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
        // GET_PROP rd=obj 是 use，结果写 a 槽
        let inst = Inst::new(OpCode::GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(1));
        assert_eq!(inst.use_regs().as_slice(), &[0, 2]);
    }

    #[test]
    fn get_prop_dynamic_def_is_b_slot() {
        // GET_PROP_DYNAMIC rd=obj 是 use，结果写 b 槽
        let inst = Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(2));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1]);
    }

    #[test]
    fn ic_get_prop_a_slot_is_use_and_def() {
        // IC_GET_PROP a 槽既是对象 use 又是结果 def
        let inst = Inst::ic_get(Operand::Reg(1), Operand::Reg(2));
        assert_eq!(inst.def_reg(), Some(1));
        let uses = inst.use_regs();
        assert_eq!(uses.as_slice(), &[1, 2]);
    }

    #[test]
    fn call_def_is_implicit_reg0_with_arg_range() {
        // CALL rd=callee 是 use，结果隐式写 reg 0；参数 b..b+nargs 连续
        let inst = Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn call_native_def_is_implicit_reg0() {
        let inst = Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 1);
        assert_eq!(inst.def_reg(), Some(0));
        assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2]);
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
        // HALT 返回 regs[0]，顶层 LOAD_VAR(None, r) 链靠它保活
        let inst = Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None);
        assert_eq!(inst.def_reg(), None);
        assert_eq!(inst.use_regs().as_slice(), &[0]);
    }

    #[test]
    fn spread_call_def_and_uses() {
        // CALL_SPREAD：结果隐式写 reg 0；use = callee + this + ext 有序实参字（静态/spread 同编）
        let call = Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[5, 0x8000_0000 | 9, 300]);
        assert_eq!(call.def_reg(), Some(0));
        assert_eq!(call.use_regs().as_slice(), &[0, 1, 5, 9, 300]);

        // NEW_EXPRESSION_SPREAD：rd 即 def；use = ctor + 实参字
        let ne = Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[7, 0x8000_0000 | 12]);
        assert_eq!(ne.def_reg(), Some(3));
        assert_eq!(ne.use_regs().as_slice(), &[0, 7, 12]);

        // SUPER_CALL_SPREAD：无 callee/this 槽
        let sc = Inst::super_call_spread(Operand::Reg(3), &[4, 0x8000_0000 | 8]);
        assert_eq!(sc.def_reg(), Some(3));
        assert_eq!(sc.use_regs().as_slice(), &[4, 8]);
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
        // None 槽与 lower.rs operand_to_u8 一致，统一映射物理 reg 0
        let lv = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::None, Operand::None);
        assert_eq!(lv.use_regs().as_slice(), &[0]);

        let cg = Inst::new(OpCode::CELL_GET, Operand::Reg(0), Operand::None, Operand::Imm(0));
        assert_eq!(cg.use_regs().as_slice(), &[0]);
    }

    #[test]
    fn template_str_parses_expr_regs_from_ext() {
        // ext[0] 跳过；后续 seg>>31==1 则低 8 位是 expr_reg（emit 8 位编码）
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
    fn spread_object_def_rd_uses_rd_and_a() {
        // SPREAD_OBJECT 原地改目标对象：def=rd，use=[rd, a]，不可删除（有观察副作用）
        let inst = Inst::spread_object(Operand::Reg(3), Operand::Reg(7));
        assert_eq!(inst.def_reg(), Some(3));
        assert_eq!(inst.use_regs().as_slice(), &[3, 7]);
        let f = IRFunction::new();
        assert!(!inst.is_pure(&f));
    }

    #[test]
    fn create_arguments_defs_rd_no_register_use_impure() {
        // CREATE_ARGUMENTS 写 rd；实参从 spill 栈读取，无寄存器 use；
        // 创建对象有观察副作用（分配的 arguments 可被外部观察到），不可删除。
        let inst = Inst::create_arguments(Operand::Reg(6));
        assert_eq!(inst.def_reg(), Some(6));
        assert!(inst.use_regs().is_empty());
        let f = IRFunction::new();
        assert!(!inst.is_pure(&f));
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

    #[test]
    fn regalloc_opcodes_def_use_contract() {
        let mov = Inst::inst_mov(Operand::Reg(3), Operand::Reg(7));
        assert_eq!(mov.def_reg(), Some(3));
        assert_eq!(mov.use_regs().as_slice(), &[7]);

        let spill = Inst::inst_spill(Operand::Reg(5), 42);
        assert_eq!(spill.def_reg(), None);
        assert_eq!(spill.use_regs().as_slice(), &[5]);

        let unspill = Inst::inst_unspill(Operand::Reg(9), 42);
        assert_eq!(unspill.def_reg(), Some(9));
        assert!(unspill.use_regs().is_empty());
    }

    // ── is_pure 契约测试（纯/非纯族代表 opcode）──

    #[test]
    fn pure_ops_are_deletable() {
        let f = IRFunction::new();
        // 算术 / 比较 / 位 / 逻辑
        assert!(Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::SUB, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::MUL, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::NEG, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::EQ, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::STRICT_EQ, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::LT, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::BIT_AND, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::BIT_NOT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::AND, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::OR, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(Inst::new(OpCode::NOT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::NULLISH, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        // 分配/常量/空操作
        assert!(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None).is_pure(&f));
        assert!(Inst::load_const(Operand::Reg(1), 0).is_pure(&f));
        assert!(Inst::new(OpCode::VOID, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::TYPEOF, Operand::Reg(1), Operand::Reg(2), Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
        // 闭包/单元读取
        assert!(Inst::create_closure(Operand::Reg(1), 0).is_pure(&f));
        assert!(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(1), Operand::Imm(0), Operand::None).is_pure(&f));
        assert!(Inst::new(OpCode::CELL_GET, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
        // 模板字符串
        assert!(Inst::template_str(Operand::Reg(1), 1, 10, &[0x1234]).is_pure(&f));
        // RegAlloc 辅助：MOV 纯
        assert!(Inst::inst_mov(Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        // LOAD_VAR 普通（a=Reg）纯
        assert!(Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::Reg(2), Operand::None).is_pure(&f));
        // STORE_VAR 一律有副作用（顶层变量赋值全局可观察，不删）
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(1)).is_pure(&f));
    }

    #[test]
    fn impure_ops_are_not_deletable() {
        let f = IRFunction::new();
        // getter / 写对象属性
        assert!(!Inst::ic_get(Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::ic_set(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::SET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::SUPER_GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::DEFINE_ACCESSOR, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::DELETE_PROP_STATIC, Operand::Reg(0), Operand::Reg(0), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::DELETE_PROP_DYNAMIC, Operand::Reg(0), Operand::None, Operand::Reg(2)).is_pure(&f));
        // 调用 / 异常 / 控制流
        assert!(!Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0).is_pure(&f));
        assert!(!Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0).is_pure(&f));
        assert!(!Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 0).is_pure(&f));
        assert!(!Inst::super_call(Operand::Reg(3), Operand::Reg(1), 0).is_pure(&f));
        assert!(!Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[3]).is_pure(&f));
        assert!(!Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[2]).is_pure(&f));
        assert!(!Inst::super_call_spread(Operand::Reg(3), &[2]).is_pure(&f));
        assert!(!Inst::new(OpCode::CREATE_REGEXP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::THROW, Operand::Reg(2), Operand::None, Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None).is_pure(&f));
        assert!(!Inst::jmp(9).is_pure(&f));
        assert!(!Inst::jmp_if_false(3, 9).is_pure(&f));
        assert!(!Inst::try_begin(9).is_pure(&f));
        assert!(!Inst::try_finally_begin(9).is_pure(&f));
        // 迭代器 / 复合更新 / 写共享状态
        assert!(!Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(0), Operand::None, Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(0), Operand::None, Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::INC_PRE, Operand::Reg(0), Operand::Reg(0), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::COMPOUND_ADD, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::MEMBER_INC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::COMPOUND_MEMBER_ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7).is_pure(&f));
        assert!(!Inst::new(OpCode::INSTANCEOF, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::IN, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::GET_PRIVATE, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
        assert!(!Inst::new(OpCode::SET_HOME_OBJECT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
        assert!(!Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
        assert!(!Inst::new(OpCode::MAKE_CELL, Operand::Reg(5), Operand::None, Operand::None).is_pure(&f));
        // RegAlloc 辅助：SPILL/UNSPILL 有副作用（删 SPILL 后 UNSPILL 读陈旧值）
        assert!(!Inst::inst_spill(Operand::Reg(5), 42).is_pure(&f));
        assert!(!Inst::inst_unspill(Operand::Reg(9), 42).is_pure(&f));
        // 占位 opcode 防御性有副作用
        assert!(!Inst::new(OpCode::SWITCH_TABLE, Operand::None, Operand::None, Operand::None).is_pure(&f));
    }

    #[test]
    fn store_var_all_branches_are_impure() {
        // STORE_VAR 写目标变量寄存器，顶层/模块作用域变量全局可观察，一律不删
        // （const guard 抛错、普通 var/let 赋值都可能改变外部观察状态）
        let f = IRFunction::new();
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
        assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(1)).is_pure(&f));
    }

    #[test]
    fn load_var_this_derived_constructor_special_case() {
        // a==This + is_derived_constructor 时读 this 可能抛 ReferenceError，不可删
        let mut derived = IRFunction::new();
        derived.is_derived_constructor = true;
        let non_derived = IRFunction::new();

        let this_load = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::This, Operand::None);
        assert!(!this_load.is_pure(&derived));
        assert!(this_load.is_pure(&non_derived));

        // a=Reg 普通 LOAD_VAR 在 derived 函数中也纯
        let reg_load = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::Reg(2), Operand::None);
        assert!(reg_load.is_pure(&derived));
    }
}
