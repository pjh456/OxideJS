use crate::vm::Vm;
use crate::{vm_error, vm_trace};
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{make_int_key, make_well_known_symbol_key};
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
        let global = self.realm.session.global_object();
        if let Some(val) = self.resolve_property(global, si) {
            self.regs[rd] = val;
            Ok(false)
        } else {
            // SAFETY: key_val 已校验为字符串值；错误消息走 lossy 文本（不可解析名
            // 含孤立 surrogate 时映射为 FFFD，仅为展示用途）。
            let name = unsafe { (*key_val.as_string_ptr()).as_lossy_str() };
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
        let global = self.realm.session.global_object();
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
                // [[IsHTMLDDA]] 宿主对象按 "undefined" 报告（B.3.4，Type 仍是 object）；
                // 宿主对象可调用，判定须先于函数分支，typeof 仍报 "undefined"。
                // 其余函数对象归为 "function"（语言类型是 object，typeof 有专属分支）。
                let obj = unsafe { &*val.as_js_object_ptr() };
                if obj.is_html_dda_obj() {
                    0
                } else if obj.is_function() {
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
    /// 1. 逐键直读模块 si 侧表（模块装载期 `activate_immutables` 已预 intern，
    ///    与逐属性 SET_PROP 路径逐字节同键），链式 `make_shape` 累加 shape。
    /// 2. 分配对象、预填充 `nprops` 个数据槽、`generation += nprops`（与逐属性路径终值一致）。
    ///
    /// # 边界与前提
    /// - `nprops = 0` 时退化为普通空对象（无 ext 键表）。
    /// - 侧表条目缺失（防御面，正常路径不出现）回退旧路径按键值重推 si。
    #[inline(always)]
    pub(crate) fn dispatch_new_object(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let nprops = opcode::a(instr) as usize;
        vm_trace!("NEW_OBJECT rd={} nprops={}", rd, nprops);
        let proto_ptr = &*self.realm.object_prototype as *const JsObject as *mut JsObject;
        let mut shape_id = EMPTY_SHAPE_ID;
        if nprops > 0 {
            let key_idxs: Vec<u32> = self.bytecode[self.pc..self.pc + nprops].to_vec();
            self.pc += nprops;
            // si 侧表与 immutables 同点激活：活动模块的侧表此刻必已填充。
            let si_table = self.active_table().si_tables[self.active_flat_id as usize]
                .get()
                .expect("活动模块的 si 侧表已随不可变常量激活同步填充");
            let key_si: Vec<Option<u32>> =
                key_idxs.iter().map(|k| si_table.get(*k as usize).copied().flatten()).collect();
            for (key_idx, si_opt) in key_idxs.iter().zip(key_si.iter()) {
                let si = match si_opt {
                    Some(si) => *si,
                    None => {
                        let imm = self.immutables();
                        let key_val = imm.get(*key_idx as usize).copied().unwrap_or(JsValue::undefined());
                        self.property_key_si(key_val)?
                    }
                };
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
        let proto_ptr = &*self.realm.object_prototype as *const JsObject as *mut JsObject;
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr));
        let obj_ptr = self.alloc_session_object(obj);
        self.regs[rd] = JsValue::object(obj_ptr as *mut u8);
        Ok(())
    }

    /// 创建 arguments 对象：索引属性取当前帧（或 inline 同步调用）的完整实参，
    /// 附 length / callee / @@iterator 属性。索引与形参不同步（mapped 同步语义
    /// 未实现）；callee 按 strict ‖ !simple 分流为受限访问器或数据属性。
    ///
    /// # 边界与前提
    /// - 实参区由统一压帧入口 `push_bytecode_frame` 在推帧时写入 spill 栈；
    ///   frames 为空（inline 同步调用）时读 `inline_args_*`。
    /// - 索引属性用 shape 槽存储（普通对象），length 为不可枚举数据属性。
    pub(crate) fn dispatch_create_arguments(&mut self, rd: usize) -> Result<(), String> {
        let (base, count) = match self.frames.last() {
            Some(frame) => (frame.arguments_base, frame.arguments_count),
            None => (self.inline_args_base, self.inline_args_count),
        };
        let proto_ptr = &*self.realm.object_prototype as *const JsObject as *mut JsObject;
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

        // callee：按 strict 与 simple 参数列表四分流（规范 CreateMappedArgumentsObject /
        // CreateUnmappedArgumentsObject）：strict ‖ !simple → 受限访问器（get/set
        // 共享 %ThrowTypeError%，访问即抛）；sloppy simple → 数据属性（值 = 当前
        // 函数本身，可写、不可枚举、可配置）。
        let callee_si = self.kernel_core.perm_interner().intern("callee").0;
        let callee = self.current_callee().unwrap_or(JsValue::undefined());
        let strict = self.current_strict();
        let simple = self.active_module().is_some_and(|m| m.has_simple_params);
        if strict || !simple {
            // SAFETY: 指针由绑定层在 session 构造期写入，session 存活期内有效。
            let thrower_ptr = self.realm.session.builtin_world().throw_type_error.get();
            if thrower_ptr.is_null() {
                // 绑定层尚未写入共享对象（正常路径不出现）：退化为数据属性保调用可用。
                if let Err(msg) =
                    self.define_data_property(obj, callee_si, callee, PropAttributes::new(true, false, true))
                {
                    return self.raise_error_kind("TypeError", &msg);
                }
            } else {
                // SAFETY: thrower_ptr 由绑定层写入的存活对象，转 *mut 供 from_js_object 消费。
                let thrower_val = JsValue::from_js_object(unsafe { &*thrower_ptr } as *const JsObject as *mut JsObject);
                if let Err(msg) = self.define_accessor_property(
                    obj,
                    callee_si,
                    thrower_val,
                    thrower_val,
                    PropAttributes::new(false, false, false),
                ) {
                    return self.raise_error_kind("TypeError", &msg);
                }
            }
        } else if let Err(msg) =
            self.define_data_property(obj, callee_si, callee, PropAttributes::new(true, false, true))
        {
            return self.raise_error_kind("TypeError", &msg);
        }

        // sloppy 函数：own caller/arguments 数据属性（ES5 遗留，node 行为）——
        // caller = 调用方函数（帧栈次顶帧 callee），arguments = 本对象。strict
        // 函数无 own caller/arguments，经原型链解析到 FP 受限访问器（访问即抛）。
        if !strict && callee.is_object() {
            let caller = self
                .frames
                .get(self.frames.len().saturating_sub(2))
                .map(|f| f.callee)
                .unwrap_or(JsValue::undefined());
            let callee_obj = unsafe { &mut *callee.as_js_object_ptr() };
            let caller_si = self.kernel_core.perm_interner().intern("caller").0;
            let arguments_si = self.kernel_core.perm_interner().intern("arguments").0;
            let data_attrs = PropAttributes::new(true, false, true);
            if let Err(msg) = self.define_data_property(callee_obj, caller_si, caller, data_attrs) {
                return self.raise_error_kind("TypeError", &msg);
            }
            if let Err(msg) =
                self.define_data_property(callee_obj, arguments_si, JsValue::from_js_object(obj_ptr), data_attrs)
            {
                return self.raise_error_kind("TypeError", &msg);
            }
        }

        // @@iterator：与 Array.prototype[Symbol.iterator] 共享同一函数对象，
        // 使 arguments 对象可迭代（可写、不可枚举、可配置）。
        let sym_iter_si = make_well_known_symbol_key(0);
        // SAFETY: array_proto 是 perm 层内置对象（BuiltinWorld 持有），地址稳定且
        // 跨 epoch/session 存活，本调用期间无 GC 搬移。
        let array_proto = self.realm.session.builtin_world().array_proto.as_ptr();
        let iter_val = match self.ordinary_get(unsafe { &*array_proto }, sym_iter_si, JsValue::undefined()) {
            Ok(v) => v,
            Err(msg) => return self.raise_error_kind("TypeError", &msg),
        };
        if let Err(msg) = self.define_data_property(obj, sym_iter_si, iter_val, PropAttributes::new(true, false, true))
        {
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
        let proto_ptr = self.realm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let obj_ptr =
            self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr), rest_len));
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
        let proto_ptr = self.realm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let obj = self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr), n));
        self.regs[rd] = JsValue::object(obj as *mut u8);
    }

    #[inline(always)]
    pub(crate) fn dispatch_void(&mut self, rd: usize) {
        vm_trace!("VOID rd={}", rd);
        self.regs[rd] = JsValue::undefined();
    }
}
