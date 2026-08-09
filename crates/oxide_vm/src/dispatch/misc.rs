use crate::vm::Vm;
use crate::{vm_error, vm_trace};
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_load_const(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let idx = (instr >> 16) as usize;
        vm_trace!("LOAD_CONST rd={} idx={}", rd, idx);
        let imm = self.immutables();
        if idx < imm.len() {
            self.regs[rd] = imm[idx];
            Ok(())
        } else {
            vm_error!("LOAD_CONST index {} out of bounds (len={})", idx, imm.len());
            Err(format!("constant index {idx} out of bounds"))
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_typeof(&mut self, rd: usize, a: usize) {
        vm_trace!("TYPEOF rd={} r{}={:?}", rd, a, self.regs[a]);
        let val = self.regs[a];
        let result = if val.is_undefined() {
            "undefined"
        } else if val.is_null() {
            "object"
        } else if val.is_bool() {
            "boolean"
        } else if val.is_int() || val.is_double() {
            "number"
        } else if val.is_string() {
            "string"
        } else if val.is_symbol() {
            "symbol"
        } else if val.is_object() {
            let obj = unsafe { &*val.as_js_object_ptr() };
            if obj.is_function() {
                "function"
            } else {
                "object"
            }
        } else {
            "undefined"
        };
        self.regs[rd] = self.new_string(result);
    }

    /// ToObject：rd 原地转换。对象直接返回；null/undefined 抛 TypeError；
    /// 其余原始值包装为对应包装对象（with 对象环境需要）。
    #[inline(always)]
    pub(crate) fn dispatch_to_object(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("TO_OBJECT rd={} r{}={:?}", rd, rd, self.regs[rd]);
        let val = self.regs[rd];
        if val.is_object() {
            return Ok(());
        }
        if val.is_null() || val.is_undefined() {
            // 抛 JS 异常而非返回 Err：外围 try/catch 须能捕获（对象解构空 pattern）。
            self.raise_error_kind("TypeError", "Cannot convert null or undefined to object")?;
            return Ok(());
        }
        let obj_val = oxide_runtime_api::to_object(val, self)?;
        self.regs[rd] = obj_val;
        Ok(())
    }

    pub(crate) fn dispatch_load_var(&mut self, rd: usize, a: usize) -> Result<bool, String> {
        vm_trace!("LOAD_VAR rd={} r{}={:?}", rd, a, self.regs[a]);
        if a == 254
            && self.frames.last().map(|frame| frame.is_derived_constructor).unwrap_or(false)
            && self.regs[a].is_undefined()
        {
            self.raise_error_kind("ReferenceError", "must call super constructor before using 'this'")?;
            return Ok(true);
        }
        self.regs[rd] = self.regs[a];
        Ok(false)
    }

    pub(crate) fn dispatch_store_var(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        vm_trace!("STORE_VAR r{}={:?} const={}", rd, self.regs[a], b);
        if b != 0 {
            // const guard：检查目标槽是否已初始化。
            if !self.regs[rd].is_undefined() {
                self.raise_error_kind("TypeError", "Assignment to constant variable")?;
                return Ok(true);
            }
        }
        self.regs[rd] = self.regs[a];
        Ok(false)
    }

    /// SPILL：`regs[rd]` → `spill_stack[帧基址 + slot]`。
    ///
    /// ext 字为 slot（u16）；帧基址 = 当前 CallFrame.spill_offset（顶层模块无帧为 0）。
    /// # 边界与前提
    /// - slot 超 u16 范围返回错误（RegAlloc 保证发射在界内）。
    /// # 副作用
    /// - 写 spill 栈，可能触发 resize 扩容。
    #[inline(always)]
    pub(crate) fn dispatch_spill(&mut self, rd: usize) -> Result<(), String> {
        let slot = self.bytecode[self.pc] as usize;
        self.pc += 1;
        if slot > u16::MAX as usize {
            return Err(format!("SPILL slot {slot} out of range (max 65535)"));
        }
        let base = self.frames.last().map(|f| f.spill_offset as usize).unwrap_or(0);
        let idx = base + slot;
        if idx >= self.spill_stack.len() {
            self.spill_stack.resize(idx + 1, JsValue::undefined());
        }
        vm_trace!("SPILL r{} -> slot[{}] base={}", rd, slot, base);
        self.spill_stack[idx] = self.regs[rd];
        Ok(())
    }

    /// UNSPILL：`spill_stack[帧基址 + slot]` → `regs[rd]`。
    ///
    /// # 边界与前提
    /// - slot 超 u16 范围返回错误。
    /// - 越界读返回 undefined（防御：slot 由 RegAlloc 保证在界内）。
    #[inline(always)]
    pub(crate) fn dispatch_unspill(&mut self, rd: usize) -> Result<(), String> {
        let slot = self.bytecode[self.pc] as usize;
        self.pc += 1;
        if slot > u16::MAX as usize {
            return Err(format!("UNSPILL slot {slot} out of range (max 65535)"));
        }
        let base = self.frames.last().map(|f| f.spill_offset as usize).unwrap_or(0);
        let idx = base + slot;
        vm_trace!("UNSPILL r{} <- slot[{}] base={}", rd, slot, base);
        self.regs[rd] = if idx < self.spill_stack.len() {
            self.spill_stack[idx]
        } else {
            JsValue::undefined()
        };
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_new_object(&mut self, rd: usize) {
        vm_trace!("NEW_OBJECT rd={}", rd);
        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let obj = self
            .epoch
            .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
        self.regs[rd] = JsValue::object(obj as *mut u8);
    }

    /// 创建 arguments 对象：索引属性取当前帧（或 inline 同步调用）的完整实参，
    /// 附 length / callee 属性。第一版为 unmapped（非严格）语义，索引与形参不同步。
    ///
    /// # 边界与前提
    /// - 实参区由 `push_bytecode_frame` / `dispatch_super_call` / `dispatch_new_expression`
    ///   在推帧时写入 spill 栈；frames 为空（inline 同步调用）时读 `inline_args_*`。
    /// - 索引属性用 shape 槽存储（普通对象），length/callee 为不可枚举数据属性。
    pub(crate) fn dispatch_create_arguments(&mut self, rd: usize) -> Result<(), String> {
        let (base, count) = match self.frames.last() {
            Some(frame) => (frame.arguments_base, frame.arguments_count),
            None => (self.inline_args_base, self.inline_args_count),
        };
        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let obj_ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
        let obj = unsafe { &mut *obj_ptr };

        // 索引属性：按实参下标写入 shape 槽，属性描述符为默认（可写/可枚举/可配置）。
        for i in 0..count as usize {
            let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
            let val = self.spill_stack.get(base as usize + i).copied().unwrap_or(JsValue::undefined());
            self.set_or_create_prop_value(obj, si, val);
        }

        // length：实参个数，可写、不可枚举、可配置。
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        if let Err(msg) = self.define_data_property(
            obj,
            length_si,
            JsValue::int(count as i32),
            PropAttributes::new(true, false, true),
        ) {
            return self.raise_error_kind("TypeError", &msg);
        }

        // callee：当前执行函数，可写、不可枚举、可配置（严格模式应抛 TypeError，未支持）。
        let callee_si = self.kernel_core.perm_interner().intern("callee").0;
        let callee = self.current_callee().unwrap_or(JsValue::undefined());
        if let Err(msg) = self.define_data_property(obj, callee_si, callee, PropAttributes::new(true, false, true)) {
            return self.raise_error_kind("TypeError", &msg);
        }

        self.regs[rd] = JsValue::from_js_object(obj_ptr);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_new_array(&mut self, rd: usize, instr: u32) {
        let n = opcode::imm16(instr) as usize;
        vm_trace!("NEW_ARRAY rd={} n={}", rd, n);
        let proto_ptr = self.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let bump = self.epoch.bump();
        let obj = self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr), n, bump));
        self.regs[rd] = JsValue::object(obj as *mut u8);
    }

    #[inline(always)]
    pub(crate) fn dispatch_void(&mut self, rd: usize) {
        vm_trace!("VOID rd={}", rd);
        self.regs[rd] = JsValue::undefined();
    }
}
