use crate::vm::Vm;
use crate::{vm_error, vm_trace};
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::make_int_key;
use oxide_types::value::{JsType, JsValue};

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

    /// 未声明标识符读：查 global object 属性存在性，命中取属性值，未命中抛 ReferenceError。
    ///
    /// # 步骤
    /// 1. imm16 取常量池 key（标识符名字符串），解析为属性键 si。
    /// 2. `resolve_property` 沿 shape 链 + 原型链解析 global 属性（覆盖全部属性存储）。
    /// 3. 命中写 rd；未命中 raise ReferenceError（`{name} is not defined`）。
    ///
    /// # 边界与前提
    /// - key 常量必须是字符串（emit 侧保证）；非字符串按错误返回。
    /// - 属性存在但值为 undefined（显式赋 undefined）时返回 undefined，不抛。
    pub(crate) fn dispatch_load_global(&mut self, rd: usize, instr: u32) -> Result<bool, String> {
        let idx = (instr >> 16) as usize;
        vm_trace!("LOAD_GLOBAL rd={} idx={}", rd, idx);
        let key_val = self.immutables().get(idx).copied().unwrap_or(JsValue::undefined());
        if !key_val.is_string() {
            return Err(format!("LOAD_GLOBAL constant index {idx} is not a string key"));
        }
        let si = self.property_key_si(key_val)?;
        let global = self.session.global_object();
        if let Some(val) = self.resolve_property(global, si) {
            self.regs[rd] = val;
            Ok(false)
        } else {
            // SAFETY: key_val 已校验为字符串值，as_str 桥接 JsString 内容。
            let name = unsafe { (*key_val.as_string_ptr()).as_str() };
            self.raise_error_kind("ReferenceError", &format!("{name} is not defined"))?;
            Ok(true)
        }
    }

    /// typeof 未声明标识符读：同 [`dispatch_load_global`] 查 global object 属性，
    /// 但未命中求值为 undefined（IsUnresolvableReference）而非抛 ReferenceError。
    ///
    /// # 步骤
    /// 1. imm16 取常量池 key（标识符名字符串），解析为属性键 si。
    /// 2. `resolve_property` 沿 shape 链 + 原型链解析 global 属性。
    /// 3. 命中写 rd；未命中写 undefined。
    pub(crate) fn dispatch_load_global_typeof(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let idx = (instr >> 16) as usize;
        vm_trace!("LOAD_GLOBAL_TYPEOF rd={} idx={}", rd, idx);
        let key_val = self.immutables().get(idx).copied().unwrap_or(JsValue::undefined());
        if !key_val.is_string() {
            return Err(format!("LOAD_GLOBAL_TYPEOF constant index {idx} is not a string key"));
        }
        let si = self.property_key_si(key_val)?;
        let global = self.session.global_object();
        self.regs[rd] = self.resolve_property(global, si).unwrap_or(JsValue::undefined());
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn dispatch_typeof(&mut self, rd: usize, a: usize) {
        vm_trace!("TYPEOF rd={} r{}={:?}", rd, a, self.regs[a]);
        let val = self.regs[a];
        let kind = match val.js_type() {
            JsType::Undefined => 0,
            JsType::Null => 1,
            JsType::Bool => 2,
            JsType::Int | JsType::Double => 3,
            JsType::String => 4,
            JsType::Symbol => 5,
            JsType::BigInt => 6,
            JsType::Object => {
                // 函数对象归为 "function"（语言类型是 object，typeof 有专属分支）。
                let obj = unsafe { &*val.as_js_object_ptr() };
                if obj.is_function() {
                    7
                } else {
                    1
                }
            }
        };
        // 结果字符串复用进程级静态表（零分配、无锁、跨 VM 共享），
        // 避免每次 typeof 新建 session 字符串。
        self.regs[rd] = JsValue::string(oxide_kernel::string_forge::typeof_string_ptr(kind));
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
        // a==254 即读 this：derived 构造帧在 super() 成功前读 this 抛 ReferenceError。
        if a == 254
            && self
                .frames
                .last()
                .map(|frame| frame.is_derived_constructor && !frame.super_called)
                .unwrap_or(false)
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

    /// 创建空对象；带键常量表时（a 槽 = 属性数 ≤255，ext = 每键常量池下标）按纯静态
    /// 数据键前缀批量构造：单次链式 `make_shape` 预建整条 shape、预分配全部数据槽，
    /// 后续属性值经 SET_PROP_BATCH 纯槽写。
    ///
    /// # 步骤
    /// 1. 逐个键常量取 perm string → `property_key_si`（与逐属性 SET_PROP 路径
    ///    逐字节同键），链式 `make_shape` 累加 shape。
    /// 2. 分配对象、预填充 `nprops` 个数据槽、`generation += nprops`（与逐属性路径终值一致）。
    ///
    /// # 边界与前提
    /// - `nprops = 0` 时退化为普通空对象（无 ext 键表）。
    /// - 键常量越界取 undefined 防御（emit 保证合法下标）。
    #[inline(always)]
    pub(crate) fn dispatch_new_object(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let nprops = opcode::a(instr) as usize;
        vm_trace!("NEW_OBJECT rd={} nprops={}", rd, nprops);
        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let mut shape_id = EMPTY_SHAPE_ID;
        if nprops > 0 {
            // 先把键常量值拷出（immutables 借 self，property_key_si 需 &mut self）。
            let key_vals: Vec<JsValue> = {
                let imm = self.immutables();
                self.bytecode[self.pc..self.pc + nprops]
                    .iter()
                    .map(|w| imm.get(*w as usize).copied().unwrap_or(JsValue::undefined()))
                    .collect()
            };
            self.pc += nprops;
            for key_val in key_vals {
                let si = self.property_key_si(key_val)?;
                shape_id = self.kernel_core.shape_forge().make_shape(shape_id, si);
            }
        }
        let obj = self.alloc_object(JsObject::new_empty(shape_id, JsValue::from_js_object(proto_ptr)));
        // 预分配数据槽并同步 generation：批量一次性 +nprops，与逐属性路径的终值一致。
        let obj_ref = unsafe { &mut *obj };
        for _ in 0..nprops {
            obj_ref.push_prop(JsValue::undefined());
            obj_ref.bump_generation();
        }
        self.regs[rd] = JsValue::object(obj as *mut u8);
        Ok(())
    }

    /// 创建 session 直分的空普通对象（[[Prototype]] = Object.prototype）：与
    /// `create_function_object` 的 prototype 子对象同策略——若按 epoch 分配，写入
    /// 全局等逃逸根时晋升屏障会深克隆进 session，持有者（构造器 prototype 属性）
    /// 与克隆体指针分裂、严格相等恒 false。
    ///
    /// # 副作用
    /// - 登记 session 对象表并计入堆账目；回收由 session GC mark/sweep 承担。
    pub(crate) fn dispatch_new_session_object(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("NEW_SESSION_OBJECT rd={}", rd);
        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr));
        obj.set_session_epoch(true);
        let obj_ptr = self.gc_state.session_epoch.alloc(obj) as *mut JsObject;
        self.gc_state.session_object_ptrs.push(obj_ptr);
        // 直 session 分配计入堆账目（与 promote 同式：对象头 + 对象堆数据）。
        self.gc_state.session_bytes_allocated += std::mem::size_of::<JsObject>()
            + crate::session_gc::SessionGc::object_heap_data_bytes(unsafe { &*obj_ptr }) as usize;
        self.regs[rd] = JsValue::object(obj_ptr as *mut u8);
        Ok(())
    }

    /// 创建 arguments 对象：索引属性取当前帧（或 inline 同步调用）的完整实参，
    /// 附 length / callee 属性。第一版为 unmapped（非严格）语义，索引与形参不同步。
    ///
    /// # 边界与前提
    /// - 实参区由统一压帧入口 `push_bytecode_frame` 在推帧时写入 spill 栈；
    ///   frames 为空（inline 同步调用）时读 `inline_args_*`。
    /// - 索引属性用 shape 槽存储（普通对象），length/callee 为不可枚举数据属性。
    pub(crate) fn dispatch_create_arguments(&mut self, rd: usize) -> Result<(), String> {
        let (base, count) = match self.frames.last() {
            Some(frame) => (frame.arguments_base, frame.arguments_count),
            None => (self.inline_args_base, self.inline_args_count),
        };
        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let obj_ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
        let obj = unsafe { &mut *obj_ptr };
        obj.type_tag = JsObject::OBJ_TYPE_ARGUMENTS;

        // 索引属性：按实参下标写入 shape 槽，属性描述符为默认（可写/可枚举/可配置）。
        // 下标走整数键，保证 `arguments[0]`（property_key_si(int 0)）键等价命中。
        for i in 0..count as usize {
            let si = make_int_key(i as u32);
            let val = self.spill_stack.get(base as usize + i).copied().unwrap_or(JsValue::undefined());
            self.set_or_create_prop_value(obj, si, val);
        }

        // length：实参个数，可写、不可枚举、可配置。
        let length_si = self.length_si;
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

    /// 创建 rest 参数数组：实参区下标 ≥ `fixed_count` 的实参收集为数组存入 `rd`。
    ///
    /// # 步骤
    /// 1. 读当前帧（或 inline 同步调用）的实参区基址与个数。
    /// 2. 实参个数超出固定形参数的部分作为数组元素写入。
    ///
    /// # 边界与前提
    /// - 实参来源与 CREATE_ARGUMENTS 相同（统一压帧入口 `push_bytecode_frame` 写入 spill 栈）。
    /// - 实参不足 `fixed_count` 时数组为空。
    pub(crate) fn dispatch_create_rest_array(&mut self, rd: usize, fixed_count: usize) -> Result<(), String> {
        let (base, count) = match self.frames.last() {
            Some(frame) => (frame.arguments_base, frame.arguments_count),
            None => (self.inline_args_base, self.inline_args_count),
        };
        let n = count as usize;
        let rest_len = n.saturating_sub(fixed_count);
        let proto_ptr = self.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let bump = self.epoch.bump();
        let obj_ptr =
            self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr), rest_len, bump));
        let obj = unsafe { &mut *obj_ptr };
        for i in 0..rest_len {
            let val = self
                .spill_stack
                .get(base as usize + fixed_count + i)
                .copied()
                .unwrap_or(JsValue::undefined());
            obj.set_prop_at(i as u32, val);
        }
        self.regs[rd] = JsValue::object(obj_ptr as *mut u8);
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
