//! dispatch 主循环：逐指令解码与 156 个 OpCode 臂分派、执行期 GC 安全点、
//! 步数/分配上限采样与二元运算宏 `binary_arith!`。

use num_traits::Zero;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;

use super::Vm;
use crate::{vm_error, vm_trace, vm_warn};

macro_rules! binary_arith {
    ($self:ident, $a:expr, $b:expr, $rd:expr, $op:tt, $check_zero:expr) => {{
        let lv = $self.regs[$a];
        let rv = $self.regs[$b];
        if lv.is_int() && rv.is_int() {
            $self.regs[$rd] = JsValue::float(lv.as_int() as f64 $op rv.as_int() as f64);
        } else if lv.is_bigint() && rv.is_bigint() {
            let l = $self.bigint_value(lv).clone();
            let r = $self.bigint_value(rv).clone();
            if r.is_zero() && $check_zero {
                // 仅除/模对 BigInt 零除数抛 RangeError；SUB/MUL 等其余二元运算
                // 遇到 0n 必须正常计算（Number 路径走 f64 inf/NaN）。
                $self.raise_error_kind("RangeError", "Division by zero")?;
                $self.regs[$rd] = JsValue::undefined();
            } else {
                $self.regs[$rd] = $self.new_bigint(l $op r);
            }
        } else {
            // 注意：混合 BigInt/Number 检查不能在此前置（对象操作数如
            // `{valueOf: () => 2n}` 需先 ToPrimitive 再判定），统一放 coerce 之后。
            let l = $self.coerce_primitive_bounded(lv, false)?;
            let r = $self.coerce_primitive_bounded(rv, false)?;
            if l.is_bigint() && r.is_bigint() {
                // 包装对象 coerce 后暴露双 BigInt（如 Object(2n) / 2n）。
                let lv = $self.bigint_value(l).clone();
                let rv = $self.bigint_value(r).clone();
                if rv.is_zero() && $check_zero {
                    $self.raise_error_kind("RangeError", "Division by zero")?;
                    $self.regs[$rd] = JsValue::undefined();
                } else {
                    $self.regs[$rd] = $self.new_bigint(lv $op rv);
                }
            } else if l.is_bigint() != r.is_bigint() {
                // 包装对象 coerce 后暴露 BigInt 混合（如 Object(1n) - 1）。
                $self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions")?;
                $self.regs[$rd] = JsValue::undefined();
            } else {
                let ln = coercion::to_number(l);
                let rn = coercion::to_number(r);
                $self.regs[$rd] = JsValue::float(ln $op rn);
            }
        }
    }}
}

impl Vm {
    pub(crate) fn dispatch(&mut self) -> Result<JsValue, String> {
        // config.max_steps / max_alloc_bytes 逐指令只读且循环内不变：提到循环外，
        // 免每次经 kernel_core Arc 指针追寻读取（热点内仅有的 config 访问）。
        let max_steps = self.kernel_core.config.max_steps;
        let max_alloc_bytes = self.kernel_core.config.max_alloc_bytes;
        let mut steps: u64 = 0;
        // 小重入泵送盲区：native 终端循环泵送短 JS 重入时，顶层 steps 不推进、
        // 每次重入 dispatch 太短到不了循环内 64 指令采样点，分配上限结构性永不
        // 采样。重入边界按 hop 计数，每 64 跳强制一次顶层轻采样兜底。
        if self.native_call_depth > 0 {
            self.reentry_hops += 1;
            if self.reentry_hops & 0x3F == 0 {
                if let Some(cap) = max_alloc_bytes {
                    let used = self.run_alloc_bytes();
                    if used > cap {
                        vm_warn!(
                            "dispatch: memory limit {cap} exceeded at reentry hop {} (used {used}) at pc={}",
                            self.reentry_hops,
                            self.pc
                        );
                        return Err(format!("VM memory limit {cap} exceeded (used {used}) at pc={}", self.pc));
                    }
                }
            }
        }
        loop {
            steps += 1;
            // 执行期 GC 安全点：仅在顶层 dispatch（native_call_depth == 0）
            // 的指令边界触发——嵌套 dispatch（builtin 经 call_function_sync 重入
            // 执行 JS 回调、generator/async 恢复）期间，调用方寄存器窗口副本存于
            // inline 状态（非 GC 根），此时回收会把调用方 regs 中的活值当死值释放。
            // 返回顶层后检查恢复，存活值此时已回拷为执行根。账目未超水位时仅
            // 少量字段比较。
            if self.native_call_depth == 0 {
                // 峰值高水位：同一边界采样 session 堆账目上界
                let bytes = self.gc_state.session_bytes_allocated;
                if bytes > self.gc_state.session_bytes_peak {
                    self.gc_state.session_bytes_peak = bytes;
                }
                // 单 run 分配包络高水位：O(1) 三计数器读（双 arena + session 账目）。
                let alloc = self.run_alloc_bytes();
                if alloc > self.gc_state.run_alloc_peak {
                    self.gc_state.run_alloc_peak = alloc;
                }
                if bytes >= self.gc_state.string_gc_watermark {
                    self.maybe_collect_session_strings();
                }
                // 执行期两档收集：O(1) 包络超触发水位 → 门控（活跃/挂起 for-in）
                // + epoch 晋升收集 + session 原地 sweep（边界契约见 maybe_collect_in_run）。
                if alloc >= self.gc_state.gc_watermark {
                    self.maybe_collect_in_run();
                }
            }
            if let Some(max_steps) = max_steps {
                if steps > max_steps {
                    vm_warn!("dispatch: step limit {} exceeded at pc={}", max_steps, self.pc);
                    self.profiling.set_instruction_count(steps);
                    return Err(format!("VM step limit exceeded at pc={}", self.pc));
                }
            }
            // 单 run 分配上限：账目盲区（属性区扩容）靠两层采样兜住——轻层每
            // 64 指令读三个计数器，深层每 2^18 指令全量重算（含逐对象属性区
            // 重算）；超限 run 按步数超限同款处理（默认 skip / --no-skip 下
            // fail），防单测试 arena 高水位拖垮宿主。
            if let Some(cap) = max_alloc_bytes {
                let deep = (steps & 0x3FFFF) == 0;
                if deep || (steps & 0x3F) == 0 {
                    let used = if deep { self.run_alloc_bytes_full() as usize } else { self.run_alloc_bytes() };
                    if used > cap {
                        vm_warn!("dispatch: memory limit {cap} exceeded (used {used}) at pc={}", self.pc);
                        self.profiling.set_instruction_count(steps);
                        return Err(format!("VM memory limit {cap} exceeded (used {used}) at pc={}", self.pc));
                    }
                }
            }
            if self.pc >= self.bytecode.len() {
                let tail: Vec<String> = self
                    .bytecode
                    .iter()
                    .enumerate()
                    .rev()
                    .take(5)
                    .map(|(i, &instr)| format!("{i}:{:?}", opcode::opcode(instr)))
                    .collect();
                let fn_names: Vec<String> = self
                    .frames
                    .iter()
                    .map(|f| {
                        self.kernel_core
                            .perm_interner()
                            .lookup(f.function_name)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "?".into())
                    })
                    .collect();
                vm_error!(
                    "dispatch: program counter out of bounds pc={} len={} frames={} cell_stack={} tail={:?} fns={:?}",
                    self.pc,
                    self.bytecode.len(),
                    self.frames.len(),
                    self.cell_stack.len(),
                    tail,
                    fn_names
                );
                vm_error!("dispatch: program counter out of bounds pc={} len={}", self.pc, self.bytecode.len());
                self.profiling.instruction_count = steps;
                return Err("program counter out of bounds".into());
            }

            let instr = self.bytecode[self.pc];
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr) as usize;
            let a = opcode::a(instr) as usize;
            let b = opcode::b(instr) as usize;
            self.pc += 1;

            match op {
                OpCode::NOP => {}

                OpCode::HALT => {
                    vm_trace!("HALT: regs[0]={:?}", self.regs[0]);
                    self.profiling.set_instruction_count(steps);
                    return Ok(self.regs[0]);
                }

                OpCode::LOAD_CONST => {
                    self.dispatch_load_const(rd, instr)?;
                }

                OpCode::LOAD_GLOBAL => match self.dispatch_load_global(rd, instr) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::LOAD_GLOBAL_TYPEOF => {
                    self.dispatch_load_global_typeof(rd, instr)?;
                }

                OpCode::CREATE_CLOSURE => {
                    self.dispatch_create_closure(rd, instr)?;
                }
                OpCode::MAKE_CELL => {
                    self.dispatch_make_cell(rd, instr)?;
                }
                OpCode::MAKE_CELL_FRESH => {
                    self.dispatch_make_cell_fresh(rd, instr)?;
                }
                OpCode::CELL_GET => {
                    self.dispatch_cell_get(rd, a, b)?;
                }
                OpCode::CELL_SET => {
                    self.dispatch_cell_set(a, b)?;
                }
                OpCode::LOAD_UPVALUE => {
                    self.dispatch_load_upvalue(rd, instr)?;
                }
                OpCode::STORE_UPVALUE => {
                    self.dispatch_store_upvalue(rd, a, b)?;
                }
                OpCode::CREATE_REGEXP => match self.dispatch_create_regexp(rd, a, b) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::ADD => {
                    self.dispatch_add(rd, a, b)?;
                }

                OpCode::CONCAT_N => {
                    self.dispatch_concat_n(rd, a)?;
                }

                OpCode::SUB => {
                    binary_arith!(self, a, b, rd, -, false);
                }

                OpCode::MUL => {
                    binary_arith!(self, a, b, rd, *, false);
                }

                OpCode::DIV => {
                    binary_arith!(self, a, b, rd, /, true);
                }

                OpCode::MOD => {
                    binary_arith!(self, a, b, rd, %, true);
                }

                OpCode::EXP => {
                    self.dispatch_exp(rd, a, b)?;
                }

                OpCode::NEG => {
                    self.dispatch_neg(rd, a)?;
                }

                OpCode::BIT_AND => {
                    self.dispatch_bit_and(rd, a, b)?;
                }

                OpCode::BIT_OR => {
                    self.dispatch_bit_or(rd, a, b)?;
                }

                OpCode::BIT_XOR => {
                    self.dispatch_bit_xor(rd, a, b)?;
                }

                OpCode::SHL => {
                    self.dispatch_shl(rd, a, b)?;
                }

                OpCode::SHR => {
                    self.dispatch_shr(rd, a, b)?;
                }

                OpCode::USHR => {
                    self.dispatch_ushr(rd, a, b)?;
                }

                OpCode::BIT_NOT => {
                    self.dispatch_bit_not(rd, a)?;
                }

                OpCode::EQ => {
                    self.dispatch_eq(rd, a, b)?;
                }

                OpCode::NEQ => {
                    self.dispatch_neq(rd, a, b)?;
                }

                OpCode::LT => {
                    self.dispatch_lt(rd, a, b)?;
                }

                OpCode::GT => {
                    self.dispatch_gt(rd, a, b)?;
                }

                OpCode::LTE => {
                    self.dispatch_lte(rd, a, b)?;
                }

                OpCode::GTE => {
                    self.dispatch_gte(rd, a, b)?;
                }

                OpCode::STRICT_EQ => {
                    self.dispatch_strict_eq(rd, a, b);
                }

                OpCode::STRICT_NEQ => {
                    self.dispatch_strict_neq(rd, a, b);
                }

                OpCode::UNARY_PLUS => {
                    self.dispatch_unary_plus(rd, a)?;
                }

                OpCode::JMP => {
                    self.dispatch_jmp(instr);
                }

                OpCode::BREAK => {
                    self.dispatch_break(instr)?;
                }

                OpCode::CONTINUE => {
                    self.dispatch_continue(instr)?;
                }

                OpCode::JMP_IF_FALSE => {
                    self.dispatch_jmp_if_false(rd, instr);
                }

                OpCode::JMP_IF_TRUE => {
                    self.dispatch_jmp_if_true(rd, instr);
                }

                OpCode::JMP_IF_NULLISH => {
                    self.dispatch_jmp_if_nullish(rd, instr);
                }

                OpCode::LOAD_VAR => match self.dispatch_load_var(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::STORE_VAR => match self.dispatch_store_var(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::MOV => {
                    vm_trace!("MOV r{} = r{}", rd, a);
                    self.regs[rd] = self.regs[a];
                }

                OpCode::SPILL => {
                    self.dispatch_spill(rd)?;
                }

                OpCode::UNSPILL => {
                    self.dispatch_unspill(rd)?;
                }

                OpCode::CALL => match self.dispatch_call(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::CALL_NATIVE => {
                    self.dispatch_call_native(rd, a, b)?;
                }

                OpCode::CALL_SPREAD => match self.dispatch_call_spread(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::NEW_EXPRESSION => match self.dispatch_new_expression(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::NEW_EXPRESSION_SPREAD => match self.dispatch_new_expression_spread(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_CALL => match self.dispatch_super_call(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_CALL_SPREAD => match self.dispatch_super_call_spread(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_GET_PROP | OpCode::SUPER_STATIC_GET_PROP => {
                    match self.dispatch_super_get_prop(rd, a, b) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => return Err(e),
                    }
                }

                OpCode::SET_HOME_OBJECT => match self.dispatch_set_home_object(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DEFINE_ACCESSOR => {
                    self.dispatch_define_accessor(rd, a, b)?;
                }

                OpCode::DEFINE_ACCESSOR_DYNAMIC => {
                    self.dispatch_define_accessor_dynamic(rd, a, b)?;
                }

                OpCode::DEFINE_PROP => {
                    self.dispatch_define_prop(rd, a, b)?;
                }

                OpCode::DEFINE_GLOBAL_PROP => {
                    self.dispatch_define_global_prop(rd, a, b)?;
                }

                OpCode::DEFINE_GLOBAL_PROP_C => {
                    let key_idx = self.bytecode[self.pc] as u16;
                    self.pc += 1;
                    self.dispatch_define_global_prop_c(a, key_idx)?;
                }

                OpCode::DEFINE_GLOBAL_PROP_IF_ABSENT => {
                    self.dispatch_define_global_prop_if_absent(rd, a, b)?;
                }

                OpCode::DEFINE_GLOBAL_PROP_C_IF_ABSENT => {
                    let key_idx = self.bytecode[self.pc] as u16;
                    self.pc += 1;
                    self.dispatch_define_global_prop_c_if_absent(a, key_idx)?;
                }

                OpCode::DELETE_GLOBAL_PROP_C => {
                    let key_idx = self.bytecode[self.pc] as u16;
                    self.pc += 1;
                    self.dispatch_delete_global_prop_c(rd, a, key_idx)?;
                }

                OpCode::CAN_DECLARE_GLOBAL_FUNC => {
                    self.dispatch_can_declare_global_func(rd, b)?;
                }

                OpCode::DEFINE_GLOBAL_FUNC_BIND => {
                    self.dispatch_define_global_func_bind(rd, a, b)?;
                }

                OpCode::DEFINE_PROP_ATTRS => {
                    let attrs = self.bytecode[self.pc] as u8;
                    self.pc += 1;
                    self.dispatch_define_prop_attrs(rd, a, b, attrs)?;
                }

                OpCode::DEFINE_ACCESSOR_ATTRS => {
                    let key_word = self.bytecode[self.pc];
                    let attrs = self.bytecode[self.pc + 1] as u8;
                    self.pc += 2;
                    self.dispatch_define_accessor_attrs(rd, a, b, key_word, attrs)?;
                }

                OpCode::RETURN => match self.dispatch_return(instr) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::GET_PROP
                | OpCode::SET_PROP
                | OpCode::SET_PROP_BATCH
                | OpCode::GET_PROP_DYNAMIC
                | OpCode::SET_PROP_DYNAMIC
                | OpCode::SET_ELEM
                | OpCode::GET_PRIVATE
                | OpCode::SET_PRIVATE
                | OpCode::INIT_PRIVATE
                | OpCode::PRIVATE_BRAND_IN => {
                    self.dispatch_property_op(op, rd, a, b)?;
                }

                OpCode::NEW_OBJECT => {
                    self.dispatch_new_object(rd, instr)?;
                }

                OpCode::NEW_SESSION_OBJECT => {
                    self.dispatch_new_session_object(rd)?;
                }

                OpCode::CREATE_ARGUMENTS => {
                    self.dispatch_create_arguments(rd)?;
                }

                OpCode::CREATE_REST_ARRAY => {
                    let fixed_count = opcode::b(instr) as usize;
                    self.dispatch_create_rest_array(rd, fixed_count)?;
                }

                OpCode::NEW_ARRAY => {
                    self.dispatch_new_array(rd, instr);
                }

                OpCode::COMPOUND_ADD => {
                    self.dispatch_compound_add(rd, a)?;
                }

                OpCode::COMPOUND_SUB => {
                    self.dispatch_compound_sub(rd, a)?;
                }

                OpCode::COMPOUND_MUL => {
                    self.dispatch_compound_mul(rd, a)?;
                }

                OpCode::COMPOUND_DIV => {
                    self.dispatch_compound_div(rd, a)?;
                }

                OpCode::COMPOUND_MOD => {
                    self.dispatch_compound_mod(rd, a)?;
                }

                OpCode::COMPOUND_EXP => {
                    self.dispatch_compound_exp(rd, a)?;
                }

                OpCode::COMPOUND_AND => {
                    self.dispatch_compound_bit_and(rd, a)?;
                }

                OpCode::COMPOUND_OR => {
                    self.dispatch_compound_bit_or(rd, a)?;
                }

                OpCode::COMPOUND_XOR => {
                    self.dispatch_compound_bit_xor(rd, a)?;
                }

                OpCode::COMPOUND_SHL => {
                    self.dispatch_compound_shl(rd, a)?;
                }

                OpCode::COMPOUND_SHR => {
                    self.dispatch_compound_shr(rd, a)?;
                }

                OpCode::COMPOUND_USHR => {
                    self.dispatch_compound_ushr(rd, a)?;
                }

                OpCode::TYPEOF => {
                    self.dispatch_typeof(rd, a);
                }

                OpCode::TO_OBJECT => {
                    self.dispatch_to_object(rd)?;
                }

                OpCode::VOID => {
                    self.dispatch_void(rd);
                }

                OpCode::TEMPLATE_STR => {
                    self.dispatch_template_str(rd)?;
                }

                OpCode::GET_TEMPLATE_OBJECT => {
                    self.dispatch_get_template_object(rd)?;
                }

                OpCode::DELETE_PROP_STATIC => match self.dispatch_delete_prop_static(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DELETE_PROP_DYNAMIC => match self.dispatch_delete_prop_dynamic(rd, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::INSTANCEOF => {
                    self.dispatch_instanceof(rd, a, b)?;
                }

                OpCode::IN => {
                    self.dispatch_in(rd, a, b)?;
                }

                OpCode::NOT => {
                    self.dispatch_not(rd, a);
                }

                OpCode::AND => {
                    self.dispatch_and(rd, a, b);
                }

                OpCode::OR => {
                    self.dispatch_or(rd, a, b);
                }

                OpCode::NULLISH => {
                    self.dispatch_nullish(rd, a, b);
                }

                OpCode::INC_PRE => {
                    self.dispatch_inc_pre(rd, a)?;
                }

                OpCode::INC_POST => {
                    self.dispatch_inc_post(rd, a)?;
                }

                OpCode::DEC_PRE => {
                    self.dispatch_dec_pre(rd, a)?;
                }

                OpCode::DEC_POST => {
                    self.dispatch_dec_post(rd, a)?;
                }

                OpCode::MEMBER_INC
                | OpCode::MEMBER_DEC
                | OpCode::DYN_MEMBER_INC
                | OpCode::DYN_MEMBER_DEC
                | OpCode::COMPOUND_MEMBER_ADD
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
                    self.dispatch_member_op(op, rd, a, b)?;
                }

                OpCode::FOR_IN_INIT => {
                    self.dispatch_for_in_init(a)?;
                }

                OpCode::FOR_IN_NEXT => {
                    self.dispatch_for_in_next(rd)?;
                }

                OpCode::FOR_IN_DONE => {
                    self.dispatch_for_in_done(rd);
                }

                OpCode::FOR_IN_CLEANUP => {
                    self.dispatch_for_in_cleanup();
                }

                OpCode::FOR_OF_INIT => {
                    self.dispatch_for_of_init(a)?;
                }

                OpCode::FOR_OF_NEXT => {
                    self.dispatch_for_of_next(rd)?;
                }

                OpCode::FOR_OF_DONE => {
                    self.dispatch_for_of_done(rd)?;
                }

                OpCode::FOR_OF_CLOSE => {
                    self.dispatch_for_of_close()?;
                }

                OpCode::FOR_AWAIT_OF_INIT => {
                    self.dispatch_for_await_of_init(a)?;
                }

                OpCode::FOR_AWAIT_OF_NEXT => {
                    self.dispatch_for_await_of_next(rd)?;
                }

                OpCode::FOR_AWAIT_OF_DONE => {
                    self.dispatch_for_await_of_done(rd, a)?;
                }

                OpCode::FOR_AWAIT_OF_CLOSE => {
                    self.dispatch_for_await_of_close()?;
                    // 异步 IteratorClose 可能经 await 挂起（return() 的 promise），
                    // 挂起时须像 AWAIT 一样让内嵌 dispatch 返回，由恢复方快照状态。
                    if self.async_suspended || self.async_gen_suspended {
                        self.profiling.set_instruction_count(steps);
                        return Ok(JsValue::undefined());
                    }
                }

                OpCode::REST_OBJECT => {
                    self.dispatch_rest_object(rd, a, b)?;
                }

                OpCode::SPREAD_OBJECT => {
                    self.dispatch_spread_object(rd, a)?;
                }

                OpCode::THROW => match self.dispatch_throw(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::TRY_BEGIN => {
                    self.dispatch_try_begin(instr);
                }

                OpCode::TRY_END => {
                    self.dispatch_try_end();
                }

                OpCode::TRY_FINALLY_BEGIN => {
                    self.dispatch_try_finally_begin(instr);
                }

                OpCode::TRY_FINALLY_ENTER => {
                    self.dispatch_try_finally_enter();
                }

                OpCode::TRY_FINALLY_END => match self.dispatch_try_finally_end() {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::YIELD => {
                    // 生成器让出：把让出值存入信号，内嵌 dispatch 返回，恢复方快照挂起状态。
                    let value = self.regs[rd];
                    self.generator_suspended = Some(value);
                    self.profiling.set_instruction_count(steps);
                    return Ok(JsValue::undefined());
                }

                OpCode::YIELD_STAR => {
                    // `yield*` 委托：取内层迭代器并推进一步。
                    // 未 done → 挂起让出（存委托迭代器）；done → 委托完成值写 reg 0 继续外层；
                    // unwind 捕获到异常 → 继续 dispatch（已展开到 catch/finally）。
                    match self.dispatch_yield_star(rd)? {
                        crate::generator::YieldStarOutcome::Suspend(value) => {
                            self.generator_suspended = Some(value);
                            self.profiling.set_instruction_count(steps);
                            return Ok(JsValue::undefined());
                        }
                        crate::generator::YieldStarOutcome::Continue(value) => {
                            self.regs[0] = value;
                        }
                        crate::generator::YieldStarOutcome::Unwind => {}
                    }
                }

                OpCode::SUSPEND_BODY => {
                    // 生成器调用时参数初始化结束：挂起在 body 起点；正常 next() 恢复时直接穿过。
                    if self.generator_init_step {
                        self.generator_body_started = true;
                        self.profiling.set_instruction_count(steps);
                        return Ok(JsValue::undefined());
                    }
                }

                OpCode::AWAIT => {
                    // 异步帧挂起：登记恢复反应后内嵌 dispatch 返回，恢复方快照挂起状态。
                    self.dispatch_await(rd)?;
                    self.profiling.set_instruction_count(steps);
                    return Ok(JsValue::undefined());
                }

                _ => {
                    return Err(format!("opcode {op} not yet implemented"));
                }
            }
        }
    }
}
