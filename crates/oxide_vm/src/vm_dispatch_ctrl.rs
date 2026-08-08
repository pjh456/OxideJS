use crate::vm::{Completion, FrameContinuation, TryHandler, Vm};
use crate::vm_trace;
use oxide_bytecode::opcode;
use oxide_types::object::PropAttributes;
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn dispatch_call(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        let callee_reg = rd;
        let this_reg = a as u8;
        let first_arg_reg = b as u8;
        let callee = self.regs[callee_reg];

        if callee.is_object() {
            let obj_ptr = callee.as_js_object_ptr();
            if !obj_ptr.is_null() {
                let obj = unsafe { &*obj_ptr };
                if obj.is_function() {                    if obj.is_class_constructor() {
                        return self
                            .raise_type_error("class constructor cannot be invoked without 'new'")
                            .map(|_| true);
                    }
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;
                    crate::vm_debug!("CALL rd={} this={} args={} depth={}", rd, this_reg, arg_count, self.frames.len());

                    if obj.native_fn().is_some() {
                        self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)?;
                        return Ok(true);
                    } else if obj.sub_module_index() > 0 {
                        let args: Vec<JsValue> = (0..arg_count)
                            .map(|i| self.regs[first_arg_reg.wrapping_add(i as u8) as usize])
                            .collect();
                        self.push_bytecode_frame(
                            callee,
                            self.regs[this_reg as usize],
                            &args,
                            None,
                            None,
                            JsValue::undefined(),
                            FrameContinuation::None,
                        )?;
                        return Ok(true);
                    }
                }
            }
        }

        let fn_flag: u8 = if callee.is_object() {
            unsafe { (*callee.as_js_object_ptr()).is_function() as u8 }
        } else {
            2
        };
        crate::vm_debug!(
            "CALL target not callable: rd={} a={} b={} callee={:?} fn_flag={}",
            rd,
            a,
            b,
            callee,
            fn_flag
        );
        self.raise_type_error("CALL target is not callable").map(|_| true)
    }

    pub(crate) fn dispatch_call_native(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let callee_reg = rd;
        let this_reg = a as u8;
        let first_arg_reg = b as u8;
        let callee = self.regs[callee_reg];

        if !callee.is_object() {
            return self.raise_type_error("CALL_NATIVE target is not an object");
        }
        let obj_ptr = callee.as_js_object_ptr();
        if obj_ptr.is_null() {
            return self.raise_type_error("CALL_NATIVE target is null");
        }
        let obj = unsafe { &*obj_ptr };
        // 目标为普通 JS 函数（如用户覆盖了 builtin 名）：回退到通用调用路径，
        // 复用 dispatch_call 的 JS 函数调用逻辑而非强制走 native。
        // pc 此刻指向 ext 字（与 CALL 指令一致），dispatch_call 直接读取。
        if obj.is_function() && obj.native_fn().is_none() {
            return self.dispatch_call(rd, a, b).map(|_| ());
        }
        if !obj.is_function() || obj.native_fn().is_none() {
            return self.raise_type_error("CALL_NATIVE target is not a native function");
        }

        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;
        crate::vm_debug!("CALL_NATIVE rd={} args={}", rd, arg_count);
        self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)
    }

    pub(crate) fn dispatch_define_accessor(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("DEFINE_ACCESSOR rd={} getter={} setter={}", rd, a, b);
        let prop_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        if prop_idx >= self.immutables().len() {
            return self.raise_type_error("DEFINE_ACCESSOR constant index out of bounds");
        }
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_ACCESSOR target is not object");
        }
        let key_val = self.immutables()[prop_idx];
        let prop_name_si = self.property_key_si(key_val);
        let getter = self.regs[a];
        let setter = self.regs[b];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let existing = self
            .get_own_property_slot(obj, prop_name_si)
            .and_then(|pos| obj.prop_meta_at(pos))
            .filter(|meta| meta.is_accessor);
        let get = if getter.is_undefined() {
            existing.map(|meta| meta.get).unwrap_or(JsValue::undefined())
        } else {
            getter
        };
        let set = if setter.is_undefined() {
            existing.map(|meta| meta.set).unwrap_or(JsValue::undefined())
        } else {
            setter
        };
        match self.define_accessor_property(obj, prop_name_si, get, set, PropAttributes::DEFAULT_DATA) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// break 完成：`crossed`（rd 槽）为 emit 词法算出的逃出 finally 域数。
    /// 逐个穿越 finally 后跳转到目标；crossed 为 0 时直接跳转。
    pub(crate) fn dispatch_break(&mut self, instr: u32) {
        let offset = opcode::offset16(instr) as isize;
        let target_pc = ((self.pc as isize) + offset - 1) as usize;
        let crossed = opcode::rd(instr) as usize;
        if let Some(finally_pc) = self.record_completion(Completion::Break { target_pc, remaining_finally: crossed }) {
            self.pc = finally_pc;
        } else {
            self.pc = target_pc;
        }
    }

    /// continue 完成：同 break，目标为循环继续位置。
    pub(crate) fn dispatch_continue(&mut self, instr: u32) {
        let offset = opcode::offset16(instr) as isize;
        let target_pc = ((self.pc as isize) + offset - 1) as usize;
        let crossed = opcode::rd(instr) as usize;
        if let Some(finally_pc) = self.record_completion(Completion::Continue { target_pc, remaining_finally: crossed }) {
            self.pc = finally_pc;
        } else {
            self.pc = target_pc;
        }
    }

    /// 记录一次控制流完成：穿越 `remaining_finally` 个 finally 体后执行完成本身。
    ///
    /// # 步骤
    /// 1. 新完成覆盖在途异常/完成（break/continue/return 是新的突然完成）。
    /// 2. 从栈顶向下扫描：逃出的 catch-only handler 弹出；正在执行的 finally 体
    ///    （`finally_active`）被覆盖弹出并计数；未进入的包裹 finally 计数后进入。
    /// 3. 计数耗尽（全部跨越的 finally 已进入或已覆盖）→ 返回 None（快速路径）。
    ///
    /// # 边界与前提
    /// - 只扫描当前帧深度（`frames.len()`）的 handler；调用者的 handler 不参与——
    ///   break/continue 词法不跨函数，return 也只逃出本函数 finally。
    ///
    /// # 副作用
    /// - 清空 `pending_exception`/`pending_completion`，弹出被逃出的 handler。
    fn record_completion(&mut self, c: Completion) -> Option<usize> {
        self.pending_exception = None;
        self.pending_error_kind = None;
        self.pending_completion = None;
        let depth = self.frames.len();
        let mut remaining = c.remaining_finally();
        loop {
            if remaining == 0 {
                return None;
            }
            let (frame_depth, finally_pc, finally_active) = {
                let Some(h) = self.try_stack.last() else {
                    return None;
                };
                (h.frame_depth, h.finally_pc, h.finally_active)
            };
            if frame_depth != depth {
                return None;
            }
            let Some(fp) = finally_pc else {
                // 逃出的 catch-only handler：丢弃防泄漏。
                self.try_stack.pop();
                continue;
            };
            remaining -= 1;
            if finally_active {
                // 正在执行本 finally 体：本完成覆盖它，弹出。
                self.try_stack.pop();
                continue;
            }
            self.try_stack.last_mut().unwrap().finally_active = true;
            self.pending_completion = Some(c.with_remaining(remaining));
            return Some(fp);
        }
    }

    /// return 完成：`crossed` = 当前帧深度内全部 finally handler 数（return 逃出整个
    /// 函数，途中被覆盖的 finally 体也在内），穿越后实际返回。
    pub(crate) fn dispatch_return(&mut self, rd: usize) -> Result<Option<JsValue>, String> {
        let result = self.regs[rd];
        crate::vm_debug!(
            "RETURN depth={} saved_pc={}",
            self.frames.len(),
            self.frames.last().map(|f| f.return_addr).unwrap_or(0)
        );
        // return 逃出整个函数：当前帧残留的纯 catch handler 一律弹出（return 不被
        // catch 捕获），finally handler 保留给下方完成穿越逐个执行。即使 emit 侧已
        // 从栈顶弹出连续 catch，这里仍扫描兜底——finally 之下的 catch 无法由
        // TRY_END 直接弹出。防 handler 泄漏到已返回函数，导致后续异常 unwind
        // 跳回死函数的 catch 形成死循环。
        self.pop_frame_catch_handlers();
        let crossed = self
            .try_stack
            .iter()
            .filter(|h| h.frame_depth == self.frames.len() && h.finally_pc.is_some())
            .count();
        if let Some(finally_pc) =
            self.record_completion(Completion::Return { value: result, remaining_finally: crossed })
        {
            self.pc = finally_pc;
            return Ok(None);
        }
        self.do_return(result)
    }

    /// 弹出当前帧内所有残留的纯 catch handler（无 finally 域），供 return 逃出
    /// 本函数时清理。保留 finally handler（供完成穿越）与其它帧的 handler。
    ///
    /// # 边界与前提
    /// - 只处理 `frame_depth == frames.len()` 的 handler：return 只逃出本帧，
    ///   调用者的 handler 必须保留。
    /// - 纯 catch 判定为 `finally_pc.is_none()`（不携带 finally 的 TRY_BEGIN）。
    fn pop_frame_catch_handlers(&mut self) {
        let depth = self.frames.len();
        let mut kept = Vec::with_capacity(self.try_stack.len());
        for h in self.try_stack.drain(..) {
            if h.frame_depth == depth && h.finally_pc.is_none() {
                continue;
            }
            kept.push(h);
        }
        self.try_stack = kept;
    }

    /// 实际执行返回：弹出当前帧并交付返回值（供 dispatch_return 与 finally 完成
    /// 恢复共用）。
    fn do_return(&mut self, result: JsValue) -> Result<Option<JsValue>, String> {
        // 兜底：return 完成恢复路径（dispatch_try_finally_end 直达）也可能携带
        // 未清理的纯 catch handler，先弹出再弹帧。
        self.pop_frame_catch_handlers();
        if let Some(frame) = self.frames.pop() {
            self.cell_stack.pop();
            let construct_result_reg = frame.construct_result_reg;
            let constructed_this = frame.constructed_this;
            let is_derived_constructor = frame.is_derived_constructor;
            let continuation = frame.continuation;
            let callee_this = self.regs[254];
            vm_trace!(
                "RETURN frame: continuation={:?}, derived={}",
                continuation,
                is_derived_constructor
            );
            self.restore_frame(frame);
            if let (Some(target_reg), Some(constructed_this)) = (construct_result_reg, constructed_this) {
                if is_derived_constructor && result.is_undefined() && callee_this.is_undefined() {
                    self.raise_error_kind("ReferenceError", "derived constructor must call super()")?;
                    return Ok(None);
                }
                self.regs[target_reg as usize] = if result.is_object() { result } else { constructed_this };
                self.regs[0] = self.regs[target_reg as usize];
                vm_trace!("RETURN constructor: target_reg={} regs[0]={:?}", target_reg, self.regs[0]);
            } else {
                match continuation {
                    FrameContinuation::None => self.regs[0] = result,
                    FrameContinuation::AccessorGet { target_reg } => {
                        self.regs[target_reg as usize] = result;
                        vm_trace!("RETURN accessor_get: target_reg={}", target_reg);
                    }
                    FrameContinuation::AccessorSet => {
                        vm_trace!("RETURN accessor_set");
                    }
                }
            }
            Ok(None)
        } else {
            vm_trace!("RETURN top-level: regs[0]={:?}", result);
            Ok(Some(result))
        }
    }

    pub(crate) fn dispatch_throw(&mut self, rd: usize) -> Result<bool, String> {
        let exc_value = self.regs[rd];
        let kind = self.thrown_error_kind(exc_value);
        vm_trace!("THROW pc={} kind={}", self.pc, kind);
        self.exception_value = Some(exc_value);
        self.pending_error_kind = Some(kind);
        self.unwind().map(|_| true)
    }

    pub(crate) fn dispatch_try_begin(&mut self, instr: u32) {
        crate::vm_trace!("TRY_BEGIN frame_depth={}", self.frames.len());
        let offset = opcode::offset16(instr) as isize;
        let catch_pc = if offset == 0 {
            None
        } else {
            Some(((self.pc as isize) + offset - 1) as usize)
        };
        self.try_stack.push(TryHandler {
            catch_pc,
            finally_pc: None,
            finally_active: false,
            frame_depth: self.frames.len(),
            for_of_depth: self.iters.for_of_iters.len(),
        });
    }

    pub(crate) fn dispatch_try_end(&mut self) {
        vm_trace!("TRY_END");
        self.try_stack.pop();
    }

    pub(crate) fn dispatch_try_finally_begin(&mut self, instr: u32) {
        let offset = opcode::offset16(instr) as isize;
        let finally_pc = ((self.pc as isize) + offset - 1) as usize;
        vm_trace!("TRY_FINALLY_BEGIN finally_pc={}", finally_pc);
        self.try_stack.push(TryHandler {
            catch_pc: None,
            finally_pc: Some(finally_pc),
            finally_active: false,
            frame_depth: self.frames.len(),
            for_of_depth: self.iters.for_of_iters.len(),
        });
    }

    /// finally 完成分发点：弹出当前 handler 后统一恢复在途异常或控制流完成。
    ///
    /// # 步骤
    /// 1. 弹出刚执行完的 finally handler。
    /// 2. 优先恢复在途异常（unwind 继续向外展开）。
    /// 3. 否则查 `pending_completion`：仍有剩余 finally 体则进入下一个（递减计数）；
    ///    耗尽则执行完成本身（break/continue 跳转目标，return 交付返回值）。
    ///
    /// # 返回值
    /// `Ok(Some(value))` = 函数返回；`Ok(None)` = 正常继续 dispatch。
    pub(crate) fn dispatch_try_finally_end(&mut self) -> Result<Option<JsValue>, String> {
        vm_trace!("TRY_FINALLY_END has_pending_exc={}", self.pending_exception.is_some());
        self.try_stack.pop();
        if self.pending_exception.is_some() && self.exception_value.is_none() {
            self.exception_value = self.pending_exception.take();
            self.unwind()?;
            return Ok(None);
        }
        self.pending_exception = None;
        let Some(c) = self.pending_completion.take() else {
            return Ok(None);
        };
        if c.remaining_finally() > 0 {
            let next = c.with_remaining(c.remaining_finally() - 1);
            if let Some(finally_pc) = self.find_next_finally() {
                self.pending_completion = Some(next);
                self.pc = finally_pc;
                return Ok(None);
            }
        }
        match c {
            Completion::Break { target_pc, .. } | Completion::Continue { target_pc, .. } => {
                self.pc = target_pc;
                Ok(None)
            }
            Completion::Return { value, .. } => self.do_return(value),
        }
    }

    /// 找下一个未被覆盖的包裹 finally 并进入（resume 路径用）：弹出途中被逃出的
    /// catch-only 与防御性的 active handler。无更多返回 None。
    fn find_next_finally(&mut self) -> Option<usize> {
        let depth = self.frames.len();
        loop {
            let (frame_depth, finally_pc, finally_active) = {
                let Some(h) = self.try_stack.last() else {
                    return None;
                };
                (h.frame_depth, h.finally_pc, h.finally_active)
            };
            if frame_depth != depth {
                return None;
            }
            let Some(fp) = finally_pc else {
                self.try_stack.pop();
                continue;
            };
            if finally_active {
                self.try_stack.pop();
                continue;
            }
            self.try_stack.last_mut().unwrap().finally_active = true;
            return Some(fp);
        }
    }
}
