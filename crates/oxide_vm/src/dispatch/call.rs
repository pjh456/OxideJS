use crate::native::NativeFn;
use crate::vm::{native_fn_ptr_to_fn, FrameArgs, FrameContinuation, Vm};
use crate::{vm_debug, vm_trace};
use oxide_builtins::iterator::make_iterator_for_value;
use oxide_builtins::{builtins_debug, builtins_trace};
use oxide_bytecode::opcode;
use oxide_runtime_api::{to_boolean, NativeResult};
use oxide_types::object::{Cell, JsObject, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    /// 把连续实参寄存器段与 this 寄存器打包成 native 调用下标表。
    ///
    /// 返回 `([u8; 257], len)`：`buf[0]` 为 this 寄存器下标，其后依次为 `first_arg_reg`
    /// 起连续递增的实参寄存器下标。数组由调用方在栈上持有，供 native 函数按下标直读
    /// 共享寄存器窗口。
    ///
    /// # 边界与前提
    /// - `arg_count` 截断为 256，超出部分不进入下标表（`len ≤ 257`）。
    /// - 实参寄存器下标按 `u8` 回绕，调用方须保证窗口内连续可用。
    #[inline(always)]
    pub(crate) fn build_native_args(first_arg_reg: u8, arg_count: usize, this_reg: u8) -> ([u8; 257], usize) {
        let mut args_buf = [0u8; 257];
        args_buf[0] = this_reg;
        let n = arg_count.min(256);
        for i in 0..n {
            args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
        }
        (args_buf, n + 1)
    }

    /// native 函数调用收口：校验调用深度、打包实参、快照 callee 槽后执行实现并按三态回写结果。
    ///
    /// native 与调用方共享扁平寄存器文件，故实现执行前把当前 callee 放进 `regs[254]`
    /// 供分发型 builtin（bind/call/apply）读取「当前 callee」，执行后恢复调用方 `this`。
    ///
    /// # 步骤
    /// 1. 调用深度达上限抛 RangeError。
    /// 2. `build_native_args` 打包实参下标表。
    /// 3. 经 `native_fn_ptr_to_fn` 取得函数项并执行；执行前后快照/恢复 `regs[254]`。
    /// 4. 按 `NativeResult` 三态回写：`Ok` 写 `regs[0]`；`Err` 以原始值走 `unwind`；
    ///    `TailCall` 目标为 native 时同步调用，否则压字节码帧。
    ///
    /// # 边界与前提
    /// - 调用方须保证 `obj` 已是 native 函数：`native_fn()` 为空时此处 unwrap 失败。
    /// - `Err` 保留异常原值：用户回调 throw 的原始值经 catch 原样收到，不包装成 Error。
    ///
    /// # 副作用
    /// - 修改 `native_call_depth`、`regs[0]`、`regs[254]`，普通调用形态清零
    ///   `constructing_native`，可能压帧或触发异常展开。
    ///
    /// # 注意事项
    /// - `native_fn_ptr_to_fn` 是 NativeFnPtr → NativeFn 的唯一强制转换点，SAFETY 前提
    ///   为 `native_fn` 由 `set_native_fn` 以合法函数项指针设置。
    #[inline(always)]
    pub(crate) fn dispatch_native_call(
        &mut self, obj: &JsObject, callee: JsValue, this_reg: u8, first_arg_reg: u8, arg_count: usize,
    ) -> Result<(), String> {
        if self.native_call_depth >= self.kernel_core.config.max_call_depth {
            return self.raise_error_kind("RangeError", "Maximum call stack size exceeded");
        }
        let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, this_reg);
        let args_slice = &args_buf[..len];
        builtins_debug!("native_call depth={} args={}", self.native_call_depth, arg_count);

        // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
        // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
        let func: NativeFn = unsafe { native_fn_ptr_to_fn(obj.native_fn().unwrap()) };
        // regs[254] 是调用方的 `this` 寄存器，同时充当分发型 builtin
        // （Function.prototype.bind/call/apply）读取的"当前 callee"槽。native 调用与
        // 调用方共享扁平寄存器文件，因此先快照调用方 `this`，在 native 执行期间暴露
        // callee，随后恢复——否则每次 native 调用都会把自己的函数对象留在调用方的
        // `this` 寄存器里。
        let saved_this = self.regs[254];
        self.regs[254] = callee;
        // 普通调用形态：清零构造形态标记，防止外层构造窗口残留误判
        // （成员式普通调用落在构造器帧内时 new.target 槽仍为类构造器）。
        self.constructing_native = false;
        self.native_call_depth += 1;
        let result = func(self, args_slice);
        self.native_call_depth -= 1;
        self.regs[254] = saved_this;
        match result {
            NativeResult::Ok(val) => {
                builtins_trace!("native_call ok depth={}", self.native_call_depth);
                self.regs[0] = val;
                Ok(())
            }
            NativeResult::Err(err_val) => {
                // 异常值原样保留：native 内部错误均为 Error 对象（走上方分支），
                // 非对象值来自用户回调 throw 的原始异常（经 last_uncaught_value
                // 恢复），按 ECMAScript 语义 catch 须收到原值，不得包装成 Error。
                let kind = self.thrown_error_kind(err_val);
                self.exception_value = Some(err_val);
                self.pending_error_kind = Some(kind);
                self.unwind()
            }
            NativeResult::TailCall { callee, this, args } => {
                if callee.is_object() {
                    let obj = unsafe { &*callee.as_js_object_ptr() };
                    // 不可调用目标（apply/call/bound 转发的 thisArg 不可调用等）抛
                    // 可捕获 TypeError；落入 push_bytecode_frame 会产生引擎级错误，
                    // 绕过 JS try/catch。
                    if !obj.is_function() || (obj.native_fn().is_none() && obj.sub_module_index() == 0) {
                        return self.raise_type_error("CALL target is not callable");
                    }
                    if obj.native_fn().is_some() {
                        // 尾调用转发为普通调用形态。
                        self.constructing_native = false;
                        match self.call_function_sync(callee, this, &args) {
                            Ok(val) => {
                                self.regs[0] = val;
                                return Ok(());
                            }
                            Err(e) => {
                                // call_function_sync 已把 native 错误展平为 String，
                                // 原始 JsValue 暂存在 last_uncaught_value——恢复为 JS 异常，
                                // 使外围 try/catch 能捕获（而非作为引擎错误上抛）；
                                // 无原值时从展平文本恢复 kind，泛型回退会把种类
                                // 降级为普通 Error。
                                let exc = self
                                    .last_uncaught_value
                                    .take()
                                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                                let kind = self.thrown_error_kind(exc);
                                self.exception_value = Some(exc);
                                self.pending_error_kind = Some(kind);
                                self.unwind()?;
                                return Ok(());
                            }
                        }
                    }
                } else {
                    return self.raise_type_error("CALL target is not callable");
                }
                self.push_bytecode_frame(
                    callee,
                    this,
                    FrameArgs::Slice(&args),
                    None,
                    None,
                    JsValue::undefined(),
                    FrameContinuation::None,
                    0,
                )
            }
        }
    }
}

impl Vm {
    pub(crate) fn dispatch_create_closure(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let sub_idx = opcode::imm16(instr) as u32;
        vm_trace!("CREATE_CLOSURE rd={} sub_idx={}", rd, sub_idx);
        // 指令操作数是发射期 flat_id，口径为执行帧字节码所属代际的平表：跨 run
        // 调用在当前代际上下文执行旧代字节码，须按执行帧的表代际解析，按当前
        // 代际会悬空（表更短）或命中他代异模块（表足够长）。
        let table = self.active_table();
        if sub_idx == 0 || (sub_idx as usize) >= table.modules.len() {
            // sub_idx 超出执行帧代际平表：定义模块缺失或发射期编号错位——按
            // 运行时错误处理而非索引越界 panic。
            return Err(format!(
                "CREATE_CLOSURE: sub_module_index {} out of bounds (max {})",
                sub_idx,
                table.modules.len()
            ));
        }
        let sub = &table.modules[sub_idx as usize];
        let is_arrow = sub.is_arrow;
        let is_class_constructor = sub.is_class_constructor;
        let is_derived_constructor = sub.is_derived_constructor;
        let needs_home_object = sub.needs_home_object;
        let upvalue_captures = sub.upvalue_captures.clone();
        let function_name = sub.function_name.clone();
        let function_length = sub.function_length;
        // 新闭包盖执行帧代际：sub_idx 与 gen 同域，跨 run 调用时新闭包仍解析
        // 回定义模块所在的原表。
        let result = self.create_function_object(
            sub_idx,
            self.active_table_gen,
            is_arrow,
            is_class_constructor,
            is_derived_constructor,
            needs_home_object,
        );
        // 函数名推断：emit 端在变量声明/对象属性赋值点设置 function_name。
        let func_obj = unsafe { &mut *result.as_js_object_ptr() };
        let length_si = self.length_si;
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        // length/name 描述符均为不可写、不可枚举、可配置（SetFunctionLength /
        // SetFunctionName 语义）；length 先于 name 定义保证属性序 [prototype, length, name]。
        let attrs = PropAttributes::new(false, false, true);
        let length_val = JsValue::int(function_length as i32);
        self.define_data_property(func_obj, length_si, length_val, attrs)?;
        let name_val = function_name
            .as_deref()
            .map(|n| self.new_string(n))
            .unwrap_or_else(|| self.new_string(""));
        self.define_data_property(func_obj, name_si, name_val, attrs)?;
        if !upvalue_captures.is_empty() {
            // 链式捕获（parent_uv_idx）：从父闭包（创建者）的 upvalues 取 cell。
            let parent_upvalues: Vec<*mut Cell> = match self.current_callee() {
                Some(callee) if callee.is_object() => unsafe { &*callee.as_js_object_ptr() }.upvalues_slice().to_vec(),
                _ => Vec::new(),
            };
            if let Some(current_cells) = self.cell_stack.last_mut() {
                let mut upvals = Box::new(Vec::with_capacity(upvalue_captures.len()));
                for capture in &upvalue_captures {
                    let cell_ptr = if let Some(puv) = capture.parent_uv_idx {
                        match parent_upvalues.get(puv as usize) {
                            Some(&ptr) => ptr,
                            // parent_uv_idx 由编译期按父 upvalue 表位置计算，越界即
                            // 捕获编码错位——Err 早暴露，避免落惰性路径读到错层 cell。
                            None => {
                                return Err(format!(
                                    "CREATE_CLOSURE: parent upvalue index {} out of bounds (parent has {})",
                                    puv,
                                    parent_upvalues.len()
                                ));
                            }
                        }
                    } else {
                        let cell_idx = capture.cell_idx as usize;
                        if cell_idx >= current_cells.len() {
                            current_cells.resize(cell_idx + 1, std::ptr::null_mut());
                        }
                        // cell 可能尚未 MAKE_CELL（hoisting 顺序：CREATE_CLOSURE 先于
                        // MAKE_CELL）——建占位 Cell，后续 MAKE_CELL 更新同一 Cell 的值，
                        // 保证 upvalue 始终指向定义方表里的稳定 cell（而非调用方表）。
                        if current_cells[cell_idx].is_null() {
                            // 占位 Cell 保持未初始化（TDZ）直到 MAKE_CELL 置位；
                            // 若绑定为 var 则其初始化 MAKE_CELL 在函数序言先于任何读取执行。
                            current_cells[cell_idx] = self.gc_state.alloc_cell(JsValue::undefined(), false);
                        }
                        current_cells[cell_idx]
                    };
                    upvals.push(cell_ptr);
                }
                let func_obj = unsafe { &mut *result.as_js_object_ptr() };
                func_obj.set_upvalues(upvals);
            }
        }
        self.regs[rd] = result;
        Ok(())
    }

    /// 初始化（或复用）当前帧调用环境的 cell：按 a 槽下标写入 `cell_stack`，b 槽决定初始化标志。
    ///
    /// # 步骤
    /// 1. 取 a 槽为 cell 下标，按需扩容 `cell_stack` 至可容纳该下标。
    /// 2. b 槽为 0 表示已初始化，非 0 为 TDZ（读写抛 ReferenceError，用于 for-in/for-of
    ///    词法头环境建模）。
    /// 3. 槽位为空时新建 cell；已有 `CREATE_CLOSURE` 建的占位 cell 则原位更新值与标志，
    ///    使闭包 upvalue 指向的同一 cell 跟随初始化。
    ///
    /// # 边界与前提
    /// - `cell_stack` 为空时 `last_mut()` unwrap 失败；调用前须已压入当前帧环境层。
    ///
    /// # 副作用
    /// - 写入/更新堆上 `Cell`（可能触发分配），改动闭包 upvalue 可见值。
    ///
    /// # 注意事项
    /// - 手写 IR 路径保留的指令，常规编译不发射，故标 `#[allow(dead_code)]`。
    #[allow(dead_code)]
    pub(crate) fn dispatch_make_cell(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        // a 槽为 cell 索引；b 槽承载未初始化标志（1 = 绑定永不被初始化，
        // 读写抛 ReferenceError，for-in/for-of 词法头 TDZ 环境建模用）。
        let cell_idx = opcode::a(instr) as usize;
        let initialized = opcode::b(instr) == 0;
        let value = self.regs[rd];
        let current = self.cell_stack.last_mut().unwrap();
        while current.len() <= cell_idx {
            current.push(std::ptr::null_mut());
        }
        if current[cell_idx].is_null() {
            // 无占位 cell：新建。
            current[cell_idx] = self.gc_state.alloc_cell(value, initialized);
        } else {
            // 更新占位 cell（CREATE_CLOSURE 已建），使闭包 upvalue 指向的
            // cell 值跟随初始化；初始化标志随指令 b 槽。
            unsafe {
                let cell = &mut *current[cell_idx];
                cell.value = value;
                cell.set_initialized(initialized);
            }
        }
        Ok(())
    }

    /// 无条件新建 cell 并替换 cell_stack[cell_idx]，值为 `regs[rd]`。
    ///
    /// 循环每迭代绑定用：把当前迭代值拷入新 cell，本迭代创建的闭包经
    /// CREATE_CLOSURE 捕获新 cell（旧闭包仍指向旧 cell，值保持）。与
    /// `dispatch_make_cell` 的唯一区别是恒新建、不复用占位 cell。
    #[allow(dead_code)]
    pub(crate) fn dispatch_make_cell_fresh(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let cell_idx = opcode::imm16(instr) as usize;
        let value = self.regs[rd];
        let current = self.cell_stack.last_mut().unwrap();
        while current.len() <= cell_idx {
            current.push(std::ptr::null_mut());
        }
        current[cell_idx] = self.gc_state.alloc_cell(value, true);
        Ok(())
    }

    /// 读取当前帧 cell：空槽惰性建 cell，未初始化（TDZ）抛 ReferenceError，否则写 `regs[rd]`。
    ///
    /// # 步骤
    /// 1. 取 b 槽为 cell 下标，按需扩容 `cell_stack`。
    /// 2. 槽位为空时以 `regs[a]` 为初值新建已初始化 cell（`CREATE_CLOSURE` 占位缺失路径）。
    /// 3. cell 未初始化抛 ReferenceError；否则值写入 `regs[rd]`。
    ///
    /// # 边界与前提
    /// - cell 下标越界按需扩容，不报错。
    ///
    /// # 注意事项
    /// - 手写 IR 保留指令（同 `dispatch_make_cell`）。
    #[allow(dead_code)]
    pub(crate) fn dispatch_cell_get(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let cell_idx = b;
        let current = self.cell_stack.last_mut().unwrap();
        while current.len() <= cell_idx {
            current.push(std::ptr::null_mut());
        }
        if current[cell_idx].is_null() {
            let val = self.regs[a];
            current[cell_idx] = self.gc_state.alloc_cell(val, true);
        }
        let c = unsafe { &*current[cell_idx] };
        if !c.is_initialized() {
            return self.raise_error_kind("ReferenceError", "Cannot access variable before initialization");
        }
        self.regs[rd] = c.value;
        Ok(())
    }

    /// 写入当前帧 cell：越界或空槽静默 no-op，TDZ 写检查后写值并置已初始化。
    ///
    /// # 步骤
    /// 1. b 槽为 cell 下标；下标越界直接返回（不建槽）。
    /// 2. 槽位为空（尚无 `CREATE_CLOSURE`/`MAKE_CELL`）静默返回，不改状态。
    /// 3. cell 未初始化（声明点前）抛 ReferenceError。
    /// 4. 写入 `regs[a]` 并置 initialized。
    ///
    /// # 边界与前提
    /// - 空槽/越界均为 no-op，与读路径的惰性建 cell 不对称：写方无可用源值建槽。
    ///
    /// # 副作用
    /// - 修改堆上 `Cell` 值与初始化标志。
    ///
    /// # 注意事项
    /// - 手写 IR 保留指令（同 `dispatch_make_cell`）。
    #[allow(dead_code)]
    pub(crate) fn dispatch_cell_set(&mut self, a: usize, b: usize) -> Result<(), String> {
        let cell_idx = b;
        let src_val = self.regs[a];
        let current = self.cell_stack.last_mut().unwrap();
        if cell_idx >= current.len() {
            return Ok(());
        }
        let cell_ptr = current[cell_idx];
        if cell_ptr.is_null() {
            return Ok(());
        }
        unsafe {
            let cell = &mut *cell_ptr;
            // TDZ 写检查：cell 未初始化（捕获绑定声明点前）写抛 ReferenceError。
            if !cell.is_initialized() {
                return self.raise_error_kind("ReferenceError", "Cannot access variable before initialization");
            }
            cell.value = src_val;
            cell.set_initialized(true);
        }
        Ok(())
    }

    /// 当前执行函数的闭包对象：普通路径取 frames 栈顶帧，inline（sync 回调）路径
    /// frames 被隔离为空，取 `inline_callee`。
    pub(crate) fn current_callee(&self) -> Option<JsValue> {
        self.frames.last().map(|f| f.callee).or(self.inline_callee)
    }

    /// 读取当前闭包的 upvalue：命中 cell 判初始化后取值，未命中委托惰性建 cell。
    ///
    /// # 步骤
    /// 1. 取 imm16 为 upvalue 下标，从当前 callee 的 `upvalues` 命中 cell。
    /// 2. 命中且非空：未初始化（TDZ）抛 ReferenceError，否则值写入 `regs[rd]`。
    /// 3. 未命中/空槽（`CREATE_CLOSURE` 早于 `MAKE_CELL`）：委托
    ///    `lazy_create_upvalue_cell` 建 cell。
    ///
    /// # 边界与前提
    /// - `uv_idx` 越界或当前无 callee 时同样走惰性路径，最终回退 undefined。
    ///
    /// # 注意事项
    /// - 手写 IR 保留指令（同 `dispatch_make_cell`）。
    #[allow(dead_code)]
    pub(crate) fn dispatch_load_upvalue(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let uv_idx = opcode::imm16(instr) as usize;
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &*callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice();
                if uv_idx < upvals.len() {
                    let cell = upvals[uv_idx];
                    if !cell.is_null() {
                        let c = unsafe { &*cell };
                        if !c.is_initialized() {
                            return self
                                .raise_error_kind("ReferenceError", "Cannot access variable before initialization");
                        }
                        self.regs[rd] = c.value;
                        return Ok(());
                    }
                }
            }
        }
        // cell 尚未创建（hoisting 顺序：CREATE_CLOSURE 先于 MAKE_CELL），
        // 用调用方寄存器值惰性创建 cell。
        self.lazy_create_upvalue_cell(rd, uv_idx)
    }

    /// 为当前闭包惰性创建 upvalue cell：初值优先取调用方 cell 表（`cell_stack` 倒数
    /// 第二层）同下标 cell 的值，缺失时取 `regs[rd]`。
    ///
    /// 服务于 `CREATE_CLOSURE` 早于 `MAKE_CELL` 的 hoisting 顺序。cell 建好后写回闭包
    /// `upvalues[uv_idx]` 并把值读回 `regs[rd]`；无 callee 或下标越界时 `regs[rd]` 置 undefined。
    fn lazy_create_upvalue_cell(&mut self, rd: usize, uv_idx: usize) -> Result<(), String> {
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &mut *callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice_mut();
                if uv_idx < upvals.len() {
                    // 尝试从调用方 cell 表取初值（MAKE_CELL 在 CREATE_CLOSURE 之后执行）。
                    let val = if self.cell_stack.len() >= 2 {
                        let caller_cells = &self.cell_stack[self.cell_stack.len() - 2];
                        if uv_idx < caller_cells.len() && !caller_cells[uv_idx].is_null() {
                            unsafe { (*caller_cells[uv_idx]).value }
                        } else {
                            self.regs[rd]
                        }
                    } else {
                        self.regs[rd]
                    };
                    upvals[uv_idx] = self.gc_state.alloc_cell(val, true);
                    self.regs[rd] = unsafe { (*upvals[uv_idx]).value };
                    return Ok(());
                }
            }
        }
        self.regs[rd] = JsValue::undefined();
        Ok(())
    }

    /// 写入当前闭包的 upvalue：空槽惰性建 cell，TDZ/const 检查后写值置已初始化。
    ///
    /// # 步骤
    /// 1. b 槽为 upvalue 下标，源值为 `regs[a]`；`rd` 复用为 const 标志。
    /// 2. 命中闭包 `upvalues` 且非空：未初始化（TDZ）抛 ReferenceError，const 标志非 0
    ///    抛 TypeError，否则写值置 initialized。
    /// 3. 槽位为空（尚未建 cell）以源值新建已初始化 cell；下标越界或无 callee 静默返回。
    ///
    /// # 边界与前提
    /// - const 检查在 TDZ 检查之后，两者均先于写值。
    ///
    /// # 注意事项
    /// - 手写 IR 保留指令（同 `dispatch_make_cell`）。
    #[allow(dead_code)]
    pub(crate) fn dispatch_store_upvalue(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let const_flag = rd;
        let uv_idx = b;
        let src_val = self.regs[a];
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &mut *callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice_mut();
                if uv_idx < upvals.len() {
                    if !upvals[uv_idx].is_null() {
                        unsafe {
                            let cell = &mut *upvals[uv_idx];
                            // TDZ 写检查：cell 未初始化（捕获绑定声明点前）写抛 ReferenceError。
                            if !cell.is_initialized() {
                                self.raise_error_kind(
                                    "ReferenceError",
                                    "Cannot access variable before initialization",
                                )?;
                                return Ok(());
                            }
                            // const guard：TDZ 检查后 cell 必已初始化（存在性判定，与值无关），
                            // const 初始值为 undefined 时再赋值同样抛。
                            if const_flag != 0 {
                                self.raise_error_kind("TypeError", "Assignment to constant variable")?;
                                return Ok(());
                            }
                            cell.value = src_val;
                            cell.set_initialized(true);
                        }
                        vm_debug!("STORE_UPVALUE len={} wrote existing", upvals.len());
                    } else {
                        upvals[uv_idx] = self.gc_state.alloc_cell(src_val, true);
                    }
                }
            }
        }
        Ok(())
    }
}

impl Vm {
    pub(crate) fn dispatch_create_regexp(&mut self, rd: usize, a: usize, b: usize) -> Result<Option<JsValue>, String> {
        vm_trace!("CREATE_REGEXP rd={}", rd);
        let pat_val = self.regs[a];
        let flags_val = self.regs[b];
        let ctor_ptr = self.session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
        let ctor = unsafe { &*ctor_ptr };
        let Some(native_fn) = ctor.native_fn() else {
            self.raise_error_kind("TypeError", "RegExp constructor unavailable")?;
            return Ok(Some(JsValue::undefined()));
        };
        let saved_0 = self.regs[0];
        let saved_1 = self.regs[1];
        let saved_2 = self.regs[2];
        self.regs[0] = JsValue::undefined();
        self.regs[1] = pat_val;
        self.regs[2] = flags_val;
        let func = unsafe { native_fn_ptr_to_fn(native_fn) };
        let result = match func(self, &[0, 1, 2]) {
            NativeResult::Ok(v) => v,
            NativeResult::Err(e) => {
                // 保留原始错误对象与类型（非法正则字面量须抛 SyntaxError，非 TypeError）。
                self.exception_value = Some(e);
                self.pending_error_kind = Some(self.thrown_error_kind(e));
                return self.unwind().map(|_| None);
            }
            NativeResult::TailCall { .. } => {
                return Err(self.error_message_text("TypeError", "unexpected tail call"));
            }
        };
        self.regs[0] = saved_0;
        self.regs[1] = saved_1;
        self.regs[2] = saved_2;
        self.regs[rd] = result;
        Ok(None)
    }

    pub(crate) fn dispatch_super_call(&mut self, rd: usize, a: usize) -> Result<bool, String> {
        vm_debug!("SUPER_CALL depth={}", self.frames.len());
        let first_arg_reg = a as u8;
        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;
        // ext 高 8 位 = 调用点存活上界（0 = 未编码/全量），压帧窗口按此截断。
        let call_window = (ext >> 8) as u8;

        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super() used outside class constructor")?;
            return Ok(true);
        };
        if !frame.is_derived_constructor {
            self.raise_error_kind("ReferenceError", "super() used outside derived constructor")?;
            return Ok(true);
        }
        if frame.super_called {
            self.raise_error_kind("ReferenceError", "super() called more than once")?;
            return Ok(true);
        }
        let Some(derived_this) = frame.constructed_this else {
            self.raise_error_kind("ReferenceError", "super() without derived this")?;
            return Ok(true);
        };
        let callee = frame.callee;

        let new_target = self.regs[255];
        if !new_target.is_object() {
            self.raise_error_kind("TypeError", "super() new.target is not an object")?;
            return Ok(true);
        }
        let new_target_obj = unsafe { &*new_target.as_js_object_ptr() };
        // super 构造器取当前帧 callee（执行中的 derived 构造器）的原型，而非 newTarget：
        // newTarget 跨 SUPER_CALL 压帧保持为最外层类，取其原型在中间 derived 层会
        // 解析回同一父类，导致父帧被反复压入（多层继承栈溢出）。
        if !callee.is_object() {
            self.raise_error_kind("TypeError", "super() without constructor callee")?;
            return Ok(true);
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let super_ctor = callee_obj.proto();
        if !super_ctor.is_object() {
            self.raise_error_kind("TypeError", "super constructor is not an object")?;
            return Ok(true);
        }
        let super_obj = unsafe { &*super_ctor.as_js_object_ptr() };
        if !super_obj.is_function() {
            self.raise_error_kind("TypeError", "super constructor is not a function")?;
            return Ok(true);
        }

        if super_obj.native_fn().is_some() {
            // 实参先物化再走同步调用收口：native 调用协议把 253/254 槽固定为
            // receiver/callee，实参寄存器（颜色 ≤253）占位 253 时裸引用会被覆写
            // （与 SUPER_CALL_SPREAD 路径同形）。
            let args: Vec<JsValue> = (0..arg_count)
                .map(|i| self.regs[first_arg_reg.wrapping_add(i as u8) as usize])
                .collect();
            // 构造形态标记夹持（SUPER native 臂不写 reg255，new.target 继承
            // 外层类构造器帧，形态判定只能靠本标记）。
            let saved_constructing = self.constructing_native;
            self.constructing_native = true;
            let result = self.call_function_sync(super_ctor, derived_this, &args);
            self.constructing_native = saved_constructing;
            match result {
                Ok(val) => {
                    // super() 返回实例的 [[Prototype]] 须设为 new.target.prototype
                    //（native 构造器不知道 new.target，由调用方设置）。
                    let instance = if val.is_object() { val } else { derived_this };
                    if instance.is_object() {
                        self.set_constructed_proto(instance, new_target_obj)?;
                    }
                    self.regs[254] = instance;
                    self.regs[rd] = self.regs[254];
                    self.mark_super_called();
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, "super() call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    match self.unwind() {
                        Ok(()) => return Ok(true),
                        Err(e) => return Err(e),
                    }
                }
            }
        } else if super_obj.sub_module_index() > 0 {
            let sub_idx = super_obj.sub_module_index() as usize;
            if self.callee_module(super_obj).is_none() {
                // 上界取 super 目标自身代际平表长度（跨 run 调用时可异于当前代际）。
                return Err(format!(
                    "SUPER_CALL: sub_module_index {} out of bounds (max {})",
                    sub_idx,
                    self.tables.get(&super_obj.table_gen()).map(|t| t.modules.len()).unwrap_or(0)
                ));
            }
            // 收敛到统一压帧入口：this = derived_this（super() 把实例交予父构造器），
            // new.target 保持外层类；构造结果写回 regs[254]。
            self.push_bytecode_frame(
                super_ctor,
                derived_this,
                FrameArgs::RegRange {
                    first: first_arg_reg,
                    count: arg_count,
                },
                Some(254),
                Some(derived_this),
                new_target,
                FrameContinuation::None,
                call_window,
            )?;
            return Ok(true);
        } else {
            self.raise_error_kind("TypeError", "super constructor is not callable")?;
            return Ok(true);
        }
        Ok(false)
    }

    /// super() 返回的构造实例其 [[Prototype]] 设为 new.target.prototype。
    fn set_constructed_proto(&mut self, instance: JsValue, new_target_obj: &JsObject) -> Result<(), String> {
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(new_target_obj, proto_si) {
            if proto_val.is_object() {
                let obj = unsafe { &mut *instance.as_js_object_ptr() };
                obj.set_proto(proto_val).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// 置位当前 derived 帧的 super_called：native super 构造器同步返回后标记
    /// super() 已成功调用（字节码父构造器路径由 do_return 在返回时置位）。
    fn mark_super_called(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            if frame.is_derived_constructor {
                frame.super_called = true;
            }
        }
    }

    pub(crate) fn dispatch_super_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        vm_trace!("SUPER_GET_PROP rd={} a={} b={}", rd, a, b);
        let key_val = self.regs[b];
        let prop_name_si = self.property_key_si(key_val)?;
        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super property used outside function")?;
            return Ok(true);
        };
        if !frame.callee.is_object() {
            self.raise_error_kind("ReferenceError", "super property has no home object")?;
            return Ok(true);
        }
        let callee_obj = unsafe { &*frame.callee.as_js_object_ptr() };
        let home_object = callee_obj.home_object();
        if !home_object.is_object() {
            self.raise_error_kind("ReferenceError", "super property has no home object")?;
            return Ok(true);
        }
        let home_obj = unsafe { &*home_object.as_js_object_ptr() };
        let super_base = home_obj.proto();
        if !super_base.is_object() {
            self.regs[rd] = JsValue::undefined();
        } else {
            let super_obj = unsafe { &*super_base.as_js_object_ptr() };
            let val = self.ordinary_get_with_target(super_obj, prop_name_si, self.regs[a], rd as u8)?;
            if self.accessor_frame_target_reg.take().is_none() {
                self.regs[rd] = val;
            }
        }
        Ok(false)
    }

    pub(crate) fn dispatch_set_home_object(&mut self, rd: usize, a: usize) -> Result<bool, String> {
        vm_trace!("SET_HOME_OBJECT rd={} a={}", rd, a);
        let func_val = self.regs[rd];
        let home_val = self.regs[a];
        if !func_val.is_object() || !home_val.is_object() {
            self.raise_error_kind("TypeError", "SET_HOME_OBJECT expects function and object")?;
            return Ok(true);
        }
        let home_val = self.promote_if_needed_for_write_ptr(func_val.as_js_object_ptr(), home_val);
        let func_obj = unsafe { &mut *func_val.as_js_object_ptr() };
        if !func_obj.is_function() {
            self.raise_error_kind("TypeError", "SET_HOME_OBJECT target is not a function")?;
            return Ok(true);
        }
        func_obj.set_home_object(home_val);
        Ok(false)
    }
}

impl Vm {
    /// 解析 spread 调用系 ext：读走 `nstatic | (nspread<<8)` 首字与 `nstatic+nspread`
    /// 个有序实参字（静态字 = 寄存器号，spread 字高位标记）。
    fn read_spread_ext(&mut self) -> Vec<usize> {
        let header = self.bytecode[self.pc];
        self.pc += 1;
        let nstatic = (header & 0xFF) as usize;
        let nspread = (header >> 8) as usize;
        let mut words = Vec::with_capacity(nstatic + nspread);
        for _ in 0..nstatic + nspread {
            words.push(self.bytecode[self.pc] as usize);
            self.pc += 1;
        }
        words
    }

    /// 按源码求值序物化完整实参 Vec：静态字直接读寄存器，spread 字迭代展开。
    ///
    /// # 步骤
    /// 1. 静态实参逐个从寄存器读入；spread 源经 `make_iterator_for_value` 迭代追加。
    /// 2. 迭代中途抛错 → 当前迭代器经 return() 关闭（IteratorClose），异常恢复为 JS
    ///    异常后 unwind。
    /// 3. 展开总数超过 u16 上限（arguments_count 字段）→ RangeError。
    ///
    /// # 返回值
    /// - `Ok(Some(vec))` 成功；
    /// - `Ok(None)` 异常已展开（调用方按 continue 处理）；
    /// - `Err` 引擎错误直接传播。
    fn materialize_spread_args(&mut self, words: &[usize]) -> Result<Option<Vec<JsValue>>, String> {
        let mut args = Vec::with_capacity(words.len());
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        for &w in words {
            if w >> 31 == 1 {
                let value = self.regs[w & 0x7FFF_FFFF];
                let iterator = match make_iterator_for_value(self, value) {
                    Ok(it) => it,
                    Err(err) => {
                        self.exception_value = Some(err);
                        self.pending_error_kind = Some(self.thrown_error_kind(err));
                        return self.unwind().map(|_| None);
                    }
                };
                loop {
                    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
                    let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    let result = match self.call_function_sync(next_fn, iterator, &[]) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    if !result.is_object() {
                        return self.spread_iter_error(
                            iterator,
                            self.error_message_text("TypeError", "iterator result is not an object"),
                            "TypeError",
                        );
                    }
                    let result_obj = unsafe { &*result.as_js_object_ptr() };
                    let done = match self.ordinary_get(result_obj, done_si, result) {
                        Ok(v) => to_boolean(v),
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    if done {
                        break;
                    }
                    let val = match self.ordinary_get(result_obj, value_si, result) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    args.push(val);
                    if args.len() > u16::MAX as usize {
                        self.raise_error_kind("RangeError", "Too many arguments in function call")?;
                        return Ok(None);
                    }
                }
            } else {
                args.push(self.regs[w]);
                if args.len() > u16::MAX as usize {
                    self.raise_error_kind("RangeError", "Too many arguments in function call")?;
                    return Ok(None);
                }
            }
        }
        Ok(Some(args))
    }

    /// spread 迭代中途异常：先关闭当前迭代器（suppress return 自身错误，保留在途异常），
    /// 再把原异常（native 错误经 last_uncaught_value 恢复）重新抛出并展开。
    fn spread_iter_error(
        &mut self, iterator: JsValue, msg: String, fallback_kind: &'static str,
    ) -> Result<Option<Vec<JsValue>>, String> {
        let exc = self
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_kind_error(self, fallback_kind, &msg));
        self.close_for_of_iterator(iterator, true)?;
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        self.unwind().map(|_| None)
    }

    /// CALL_SPREAD：运行期物化完整实参后按目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_call_spread(&mut self, rd: usize, a: usize, _b: usize) -> Result<bool, String> {
        let callee = self.regs[rd];
        let this_value = self.regs[a];
        let words = self.read_spread_ext();
        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };
        if callee.is_object() {
            let obj_ptr = callee.as_js_object_ptr();
            if !obj_ptr.is_null() {
                let obj = unsafe { &*obj_ptr };
                if obj.is_function() {
                    if obj.is_class_constructor() {
                        return self
                            .raise_type_error("class constructor cannot be invoked without 'new'")
                            .map(|_| true);
                    }
                    if obj.native_fn().is_some() {
                        // spread 普通调用形态：清零构造形态标记。
                        self.constructing_native = false;
                        match self.call_function_sync(callee, this_value, &args) {
                            Ok(v) => {
                                self.regs[0] = v;
                                return Ok(false);
                            }
                            Err(_) => {
                                let exc = self
                                    .last_uncaught_value
                                    .take()
                                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, "call failed"));
                                self.exception_value = Some(exc);
                                self.pending_error_kind = Some(self.thrown_error_kind(exc));
                                return self.unwind().map(|_| true);
                            }
                        }
                    } else if obj.sub_module_index() > 0 {
                        // 按函数对象自身记录的表代际解析：表缺失（已回收）时落
                        // 普通压帧路径，由 push_bytecode_frame 按自身口径报错。
                        let sub = self.callee_module(obj);
                        // 异步生成器函数调用返回异步生成器迭代器对象。
                        if matches!(sub, Some(m) if m.is_generator && m.is_async) {
                            let gen_pc = self.pc;
                            let gen = self.create_async_generator_object(callee, this_value, &args)?;
                            // 参数初始化抛错已就地展开（unwind 改写 pc 至 catch）：异常值已
                            // 写入 catch 参数，再写结果寄存器会覆盖之——仅在初始化成功（pc 未动）
                            // 时交付生成器对象。
                            if self.pc == gen_pc {
                                self.regs[0] = gen;
                            }
                            return Ok(false);
                        }
                        // 生成器函数调用返回迭代器对象，不执行函数体。
                        if matches!(sub, Some(m) if m.is_generator) {
                            let gen_pc = self.pc;
                            let gen = self.create_generator_object(callee, this_value, &args)?;
                            // 参数初始化抛错已就地展开时不得覆盖 catch 参数（同上）。
                            if self.pc == gen_pc {
                                self.regs[0] = gen;
                            }
                            return Ok(false);
                        }
                        // 异步函数调用返回 capability promise，立即同步执行 body 到首个 await。
                        if matches!(sub, Some(m) if m.is_async) {
                            let promise = self.create_async_object(callee, this_value, &args)?;
                            self.regs[0] = promise;
                            return Ok(false);
                        }
                        self.push_bytecode_frame(
                            callee,
                            this_value,
                            FrameArgs::Slice(&args),
                            None,
                            None,
                            JsValue::undefined(),
                            FrameContinuation::None,
                            0,
                        )?;
                        return Ok(true);
                    }
                }
            }
        }
        self.raise_type_error("CALL target is not callable").map(|_| true)
    }

    /// NEW_EXPRESSION_SPREAD：物化实参后按构造函数目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_new_expression_spread(&mut self, rd: usize, a: usize, _b: usize) -> Result<bool, String> {
        let constructor_reg = a;
        let constructor = self.regs[constructor_reg];

        if !constructor.is_object() {
            return self
                .raise_type_error("NEW_EXPRESSION: constructor is not an object")
                .map(|_| true);
        }
        let ctor_ptr = constructor.as_js_object_ptr();
        if ctor_ptr.is_null() {
            return self.raise_type_error("NEW_EXPRESSION: constructor is null").map(|_| true);
        }
        let ctor_obj = unsafe { &*ctor_ptr };
        if !ctor_obj.is_function() {
            return self
                .raise_type_error("NEW_EXPRESSION: constructor is not a function")
                .map(|_| true);
        }
        if ctor_obj.is_arrow() {
            return self
                .raise_type_error("arrow functions cannot be used as constructors")
                .map(|_| true);
        }
        // native 方法（非构造器）不可 new；bound 包装除外（其构造语义转发到 target）。
        if ctor_obj.native_fn().is_some()
            && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR
            && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_BOUND
        {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }

        let words = self.read_spread_ext();
        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };

        // bound 包装：解包链后转发到最内层 target（[[Construct]] 语义）。
        if ctor_obj.type_tag == oxide_types::object::JsObject::OBJ_TYPE_BOUND {
            let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
            let new_obj = self.alloc_object(JsObject::new_empty(
                oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
                JsValue::from_js_object(proto_ptr),
            ));
            return self.dispatch_new_bound(rd, constructor, new_obj, args, 0);
        }

        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let new_obj = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        // GpFC 按构造器种类分读形：native 构造器保持裸存储读（占位 receiver
        // 不可观测，构造体自重读覆盖可观测面）；字节码构造器走传播读（getter
        // 异常原值上抛，非对象结果回落 Object.prototype）。
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        let proto_val = if ctor_obj.native_fn().is_some() {
            self.resolve_property(ctor_obj, proto_si)
        } else {
            let pc_before = self.pc;
            match self.ordinary_get(ctor_obj, proto_si, constructor) {
                Ok(v) if self.pc == pc_before => Some(v),
                Ok(_) => {
                    // getter 抛且已有 catch/finally 接手：展开已跳到落点，停构造流。
                    return Ok(true);
                }
                Err(err) => {
                    // 无处理器（或重入在途异常）：恢复原值走异常通道。
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &err));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    return self.unwind().map(|_| true);
                }
            }
        };
        if let Some(v) = proto_val {
            if v.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = v.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }
        let new_obj_val = JsValue::object(new_obj as *mut u8);

        if ctor_obj.native_fn().is_some() {
            // native 构造器：receiver 为新对象，值传递调用。newTarget 快照后置入
            // reg(255) 暴露给 native 构造器（调用后恢复原值）；构造形态标记同窗
            // 夹持（调用返回即恢复，先于结果分派，异常结局不残留）。
            let saved_new_target = self.regs[255];
            let saved_constructing = self.constructing_native;
            self.regs[255] = constructor;
            self.constructing_native = true;
            let result = self.call_function_sync(constructor, new_obj_val, &args);
            self.constructing_native = saved_constructing;
            self.regs[255] = saved_new_target;
            match result {
                Ok(v) => {
                    self.regs[rd] = if v.is_object() { v } else { new_obj_val };
                    Ok(false)
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, "constructor call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)
                }
            }
        } else if ctor_obj.sub_module_index() > 0 {
            // 按构造器自身记录的表代际解析。
            let sub = self.callee_module(ctor_obj);
            // 生成器函数不是构造器：`new g()` 抛 TypeError。
            if matches!(sub, Some(m) if m.is_generator) {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }
            // 异步函数不是构造器：`new f()` 抛 TypeError。
            if matches!(sub, Some(m) if m.is_async) {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }
            let this_value = if ctor_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.push_bytecode_frame(
                constructor,
                this_value,
                FrameArgs::Slice(&args),
                Some(rd as u8),
                Some(new_obj_val),
                constructor,
                FrameContinuation::None,
                0,
            )?;
            Ok(true)
        } else {
            let error =
                oxide_builtins::error::create_error(self, "NEW_EXPRESSION: bytecode constructors not yet supported");
            self.exception_value = Some(error);
            self.pending_error_kind = Some(self.thrown_error_kind(error));
            self.unwind().map(|_| true)
        }
    }

    /// SUPER_CALL_SPREAD：物化实参后按父构造器目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_super_call_spread(&mut self, rd: usize, _a: usize) -> Result<bool, String> {
        let words = self.read_spread_ext();

        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super() used outside class constructor")?;
            return Ok(true);
        };
        if !frame.is_derived_constructor {
            self.raise_error_kind("ReferenceError", "super() used outside derived constructor")?;
            return Ok(true);
        }
        if frame.super_called {
            self.raise_error_kind("ReferenceError", "super() called more than once")?;
            return Ok(true);
        }
        let Some(derived_this) = frame.constructed_this else {
            self.raise_error_kind("ReferenceError", "super() without derived this")?;
            return Ok(true);
        };
        let callee = frame.callee;

        let new_target = self.regs[255];
        if !new_target.is_object() {
            self.raise_error_kind("TypeError", "super() new.target is not an object")?;
            return Ok(true);
        }
        let new_target_obj = unsafe { &*new_target.as_js_object_ptr() };
        // 同 dispatch_super_call：super 构造器取当前帧 callee 的原型（newTarget
        // 保持为最外层类，取其原型在中间 derived 层会反复压入同一父帧）。
        if !callee.is_object() {
            self.raise_error_kind("TypeError", "super() without constructor callee")?;
            return Ok(true);
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let super_ctor = callee_obj.proto();
        if !super_ctor.is_object() {
            self.raise_error_kind("TypeError", "super constructor is not an object")?;
            return Ok(true);
        }
        let super_obj = unsafe { &*super_ctor.as_js_object_ptr() };
        if !super_obj.is_function() {
            self.raise_error_kind("TypeError", "super constructor is not a function")?;
            return Ok(true);
        }

        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };

        if super_obj.native_fn().is_some() {
            // 构造形态标记夹持（SUPER native 臂不写 reg255，new.target 继承
            // 外层类构造器帧，形态判定只能靠本标记）。
            let saved_constructing = self.constructing_native;
            self.constructing_native = true;
            let result = self.call_function_sync(super_ctor, derived_this, &args);
            self.constructing_native = saved_constructing;
            match result {
                Ok(val) => {
                    // super() 返回实例的 [[Prototype]] 须设为 new.target.prototype
                    let instance = if val.is_object() { val } else { derived_this };
                    if instance.is_object() {
                        self.set_constructed_proto(instance, new_target_obj)?;
                    }
                    self.regs[254] = instance;
                    self.regs[rd] = instance;
                    self.mark_super_called();
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, "super() call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)?;
                }
            }
            Ok(false)
        } else if super_obj.sub_module_index() > 0 {
            self.push_bytecode_frame(
                super_ctor,
                derived_this,
                FrameArgs::Slice(&args),
                Some(254),
                Some(derived_this),
                new_target,
                FrameContinuation::None,
                0,
            )?;
            Ok(true)
        } else {
            self.raise_error_kind("TypeError", "super constructor is not callable")?;
            Ok(true)
        }
    }
}
