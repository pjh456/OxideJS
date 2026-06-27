use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;

use crate::vm::{CallFrame, InlineSyncState, Vm};
use crate::{vm_debug, vm_info, vm_trace, vm_warn};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn call_bytecode_function_inline(
        &mut self, callee: JsValue, callee_obj: &JsObject, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        if callee_obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let sub_idx = callee_obj.sub_module_index() as usize - 1;
        vm_debug!(
            "call_bytecode_function_inline: sub_idx={}, args={}, depth={}",
            sub_idx,
            args.len(),
            self.frames.len()
        );
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "accessor sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel_core.config.max_call_depth {
            self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
            return Ok(JsValue::undefined());
        }
        if self.native_call_depth >= self.kernel_core.config.max_call_depth {
            self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
            return Ok(JsValue::undefined());
        }
        self.native_call_depth += 1;

        let subs = Arc::clone(&self.sub_modules);
        let sub = &subs[sub_idx];

        vm_trace!("call_bytecode: saving state pc={} depth={}", self.pc, self.frames.len());
        let saved = Box::new(InlineSyncState {
            regs: Box::new(self.regs),
            pc: self.pc,
            bytecode: std::mem::take(&mut self.bytecode),
            active_immutables: self.active_immutables,
            active_reg_limit: self.active_reg_limit,
            root_reg_limit: self.root_reg_limit,
            try_stack: std::mem::take(&mut self.try_stack),
            frames: std::mem::take(&mut self.frames),
            exception_value: self.exception_value.take(),
            pending_exception: self.pending_exception.take(),
            pending_error_kind: self.pending_error_kind.take(),
            for_in_iters: std::mem::take(&mut self.iters.for_in_iters),
            for_of_iters: std::mem::take(&mut self.iters.for_of_iters),
            last_for_of_result: self.iters.last_for_of_result,
            saved_bytecode_stack: std::mem::take(&mut self.saved_bytecode_stack),
            saved_immutables_stack: std::mem::take(&mut self.saved_immutables_stack),
            save_stack: std::mem::take(&mut self.save_stack),
        });

        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode = sub.bytecode.clone();
        self.activate_immutables(sub_idx + 1, &sub.constants);
        self.active_reg_limit = sub.n_registers.max(1);
        self.root_reg_limit = self.active_reg_limit;
        for i in 0..sub.n_args as usize {
            self.regs[sub.param_base as usize + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub.is_arrow { callee_obj.captured_this() } else { receiver };
        self.regs[255] = JsValue::undefined();
        for (name, reg) in &sub.builtin_reg_map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }
        let _ = callee;

        vm_trace!(
            "call_bytecode: dispatching sub_idx={} n_regs={} n_args={} arrow={}",
            sub_idx,
            sub.n_registers,
            sub.n_args,
            sub.is_arrow
        );
        let result = self.dispatch();
        self.native_call_depth -= 1;

        vm_trace!("call_bytecode: restoring state pc={} result={:?}", saved.pc, result.as_ref().ok());
        self.regs = *saved.regs;
        self.pc = saved.pc;
        self.bytecode = saved.bytecode;
        self.active_immutables = saved.active_immutables;
        self.active_reg_limit = saved.active_reg_limit;
        self.root_reg_limit = saved.root_reg_limit;
        self.try_stack = saved.try_stack;
        self.frames = saved.frames;
        self.exception_value = saved.exception_value;
        self.pending_exception = saved.pending_exception;
        self.pending_error_kind = saved.pending_error_kind;
        self.iters.for_in_iters = saved.for_in_iters;
        self.iters.for_of_iters = saved.for_of_iters;
        self.iters.last_for_of_result = saved.last_for_of_result;
        self.saved_bytecode_stack = saved.saved_bytecode_stack;
        self.saved_immutables_stack = saved.saved_immutables_stack;
        self.save_stack = saved.save_stack;

        result
    }

    pub(crate) fn restore_frame(&mut self, frame: CallFrame) {
        vm_trace!(
            "restore_frame: return_addr={} caller_reg_limit={}",
            frame.return_addr,
            frame.caller_reg_limit
        );
        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
            self.bytecode = saved_bc;
        }
        if let Some(saved_imm) = self.saved_immutables_stack.pop() {
            self.active_immutables = saved_imm;
        }
        let offset = frame.saved_reg_offset as usize;
        let len = frame.caller_reg_limit as usize;
        self.regs[..len].copy_from_slice(&self.save_stack[offset..offset + len]);
        self.save_stack.truncate(offset);
        self.regs[254] = frame.saved_this;
        self.regs[255] = frame.saved_new_target;
        self.active_reg_limit = frame.caller_reg_limit;
        self.pc = frame.return_addr;
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        vm_info!("rerun: clearing IC caches");
        self.clear_execution_state();
        self.active_reg_limit = self.root_reg_limit;
        crate::ic_helper::clear_ic_caches(&mut self.bytecode);
        self.dispatch()
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        vm_debug!("run: starting bytecode execution, {} instructions", module.bytecode.len());
        self.clear_execution_state();
        self.sub_modules = Arc::new(module.sub_modules.clone());
        // Per-run convert-once cache: slot 0 = top module, slot sub_idx+1 = sub_modules[sub_idx].
        self.immutables_cache = (0..=self.sub_modules.len()).map(|_| OnceLock::new()).collect();
        self.bytecode = module.bytecode.clone();
        self.activate_immutables(0, &module.constants);
        self.root_reg_limit = module.n_registers.max(1);
        self.active_reg_limit = self.root_reg_limit;

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.regs[254] = JsValue::undefined();

        self.dispatch()
    }

    pub(crate) fn unwind(&mut self) -> Result<(), String> {
        vm_debug!("unwind: {} try handlers on stack", self.try_stack.len());
        while let Some(handler) = self.try_stack.pop() {
            while self.frames.len() > handler.frame_depth {
                if let Some(frame) = self.frames.pop() {
                    self.restore_frame(frame);
                }
            }
            // IteratorClose any for-of loops abandoned inside this handler's scope.
            self.close_for_of_above(handler.for_of_depth);
            if let Some(finally_pc) = handler.finally_pc {
                vm_trace!("unwind: entering finally at pc={}", finally_pc);
                if self.pending_exception.is_none() {
                    self.pending_exception = self.exception_value.take();
                }
                self.try_stack.push(handler);
                self.pc = finally_pc;
                return Ok(());
            }
            if let Some(catch_pc) = handler.catch_pc {
                vm_trace!("unwind: caught at pc={}", catch_pc);
                let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
                self.regs[0] = exc;
                self.pc = catch_pc;
                return Ok(());
            }
        }
        self.close_for_of_above(0);
        while let Some(frame) = self.frames.pop() {
            self.restore_frame(frame);
        }
        let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
        // Preserve the escaping JsValue so a for-of next()-throw can re-throw the original
        // value instead of the flattened string (consumed by dispatch_for_of_done/next).
        self.last_uncaught_value = Some(exc);
        let kind_str = self.pending_error_kind.take().unwrap_or("Error");
        let exc_text = self.error_text(exc);
        let msg = if exc_text.starts_with(kind_str) {
            format!("uncaught {exc_text}")
        } else {
            format!("uncaught {kind_str}: {exc_text}")
        };
        vm_warn!("unwind: uncaught {}: {}", kind_str, exc_text);
        Err(msg)
    }
}
