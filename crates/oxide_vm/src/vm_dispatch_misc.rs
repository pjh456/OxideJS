use std::sync::Arc;

use crate::native::NativeFn;
use crate::vm::{native_fn_ptr_to_fn, CallFrame, ForInIter, FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::vm_trace;
use oxide_runtime_api::{to_boolean, to_string_full, NativeResult};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{is_private_name_key, is_symbol_key};
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn dispatch_new_expression(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        let constructor_reg = a;
        let first_arg_reg = b as u8;
        vm_trace!("NEW_EXPRESSION rd={}", rd);

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
        // native 方法（非构造器）不可 new。
        if ctor_obj.native_fn().is_some() && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }

        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;

        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let new_obj = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));

        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
            if proto_val.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = proto_val.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }

        if ctor_obj.native_fn().is_some() {
            let new_obj_val = JsValue::object(new_obj as *mut u8);
            self.regs[255] = new_obj_val;

            let mut args_buf = [0u8; 257];
            args_buf[0] = 255u8;
            for i in 0..arg_count.min(256) {
                args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
            }
            let args_slice = &args_buf[..arg_count + 1];

            let func: NativeFn = unsafe { native_fn_ptr_to_fn(ctor_obj.native_fn().unwrap()) };
            self.regs[254] = constructor;
            match func(self, args_slice) {
                NativeResult::Ok(val) => {
                    self.regs[rd] = if val.is_object() { val } else { new_obj_val };
                    Ok(false)
                }
                NativeResult::Err(err_val) => {
                    self.exception_value = Some(err_val);
                    self.pending_error_kind = Some(self.thrown_error_kind(err_val));
                    self.unwind().map(|_| true)
                }
                NativeResult::TailCall { .. } => {
                    self.raise_type_error("constructor tail call not supported").map(|_| true)
                }
            }
        } else if ctor_obj.sub_module_index() > 0 {
            let sub_idx = ctor_obj.sub_module_index() as usize;
            if sub_idx >= self.sub_modules.len() {
                return Err(format!(
                    "NEW_EXPRESSION: sub_module_index {} out of bounds (max {})",
                    sub_idx,
                    self.sub_modules.len()
                ));
            }
            // 生成器函数不是构造器：`new g()` 抛 TypeError。
            if self.sub_modules[sub_idx].is_generator {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }
            // 异步函数不是构造器：`new f()` 抛 TypeError。
            if self.sub_modules[sub_idx].is_async {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }

            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err(self.error_message_text("RangeError", "Maximum call stack size exceeded"));
            }

            let new_obj_val = JsValue::object(new_obj as *mut u8);
            let sub_bytecode = Arc::clone(&self.sub_modules[sub_idx].bytecode);
            let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
            let sub_n_registers = self.sub_modules[sub_idx].n_registers;
            let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
            let caller_reg_limit = self.active_reg_limit.max(1);
            let saved_reg_offset = self.save_stack.len() as u32;
            self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
            let saved_this = self.regs[254];
            let saved_new_target = self.regs[255];

            // 完整实参写入 spill 栈实参区（在帧的 spill 区之前），供 CREATE_ARGUMENTS 使用。
            let args_base = self.spill_stack.len() as u32;
            for i in 0..arg_count {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.spill_stack.push(self.regs[src_reg]);
            }
            let args_count = arg_count.min(u16::MAX as usize) as u16;

            for i in 0..sub_n_args {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.regs[sub_param_base + i] = self.regs[src_reg];
            }
            self.regs[254] = if ctor_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.regs[255] = constructor;

            self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
            self.saved_immutables_stack.push(self.active_immutables);

            self.frames.push(CallFrame {
                return_addr: self.pc,
                function_name: self.sub_modules[sub_idx]
                    .function_name
                    .as_deref()
                    .map(|name| self.kernel_core.perm_interner().intern(name).0)
                    .unwrap_or(0),
                caller_reg_limit,
                saved_reg_offset,
                spill_offset: self.spill_stack.len() as u32,
                arguments_base: args_base,
                arguments_count: args_count,
                saved_this,
                saved_new_target,
                callee: constructor,
                construct_result_reg: Some(rd as u8),
                constructed_this: Some(new_obj_val),
                is_derived_constructor: ctor_obj.is_derived_constructor(),
                continuation: FrameContinuation::None,
            });

            self.bytecode = sub_bytecode;
            let subs = Arc::clone(&self.sub_modules);
            self.activate_immutables(sub_idx, &subs[sub_idx].constants);
            self.cell_stack.push(Vec::with_capacity(subs[sub_idx].cells_needed as usize));

            for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
                let global = self.session.global_object();
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                    self.regs[*reg as usize] = global.get_prop_at(pos);
                }
            }

            self.active_reg_limit = sub_n_registers.max(1);
            self.pc = 0;
            Ok(true)
        } else {
            let error =
                oxide_builtins::error::create_error(self, "NEW_EXPRESSION: bytecode constructors not yet supported");
            self.exception_value = Some(error);
            self.pending_error_kind = Some(self.thrown_error_kind(error));
            self.unwind().map(|_| true)
        }
    }

    pub(crate) fn dispatch_template_str(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("TEMPLATE_STR rd={}", rd);
        let header = self.bytecode[self.pc];
        self.pc += 1;
        let segment_count = (header >> 16) as usize;
        let len_hint = (header & 0xFFFF) as usize;

        let mut result = String::with_capacity(len_hint.max(16));
        for _ in 0..segment_count {
            let seg = self.bytecode[self.pc];
            self.pc += 1;
            if (seg >> 31) == 1 {
                let reg = (seg & 0x7FFF_FFFF) as usize;
                let val = self.regs[reg];
                let s = if val.is_string() {
                    // SAFETY: val 是字符串值。
                    unsafe { (*val.as_string_ptr()).data.clone() }
                } else {
                    to_string_full(val, self)?
                };
                result.push_str(&s);
            } else {
                let const_idx = (seg & 0x7FFF_FFFF) as usize;
                let imm = self.immutables();
                if const_idx < imm.len() {
                    let val = imm[const_idx];
                    if val.is_string() {
                        // SAFETY: val 是字符串值。
                        let s = unsafe { (*val.as_string_ptr()).data.clone() };
                        result.push_str(&s);
                    }
                }
            }
        }
        self.regs[rd] = self.new_string(&result);
        Ok(())
    }

    pub(crate) fn dispatch_instanceof(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("INSTANCEOF rd={}", rd);
        let lhs_val = self.regs[a];
        let rhs_val = self.regs[b];

        if !rhs_val.is_object() {
            return self.raise_type_error("INSTANCEOF right-hand side is not callable");
        }

        let has_instance_ptr = self.session.builtin_world().sym_has_instance.as_ptr() as *mut JsObject;
        let has_instance_key = JsValue::from_js_object(has_instance_ptr);
        let has_instance_si = self.property_key_si(has_instance_key)?;

        let ctor_obj = unsafe { &*rhs_val.as_js_object_ptr() };
        let has_instance_val = self.ordinary_get(ctor_obj, has_instance_si, rhs_val)?;
        if !has_instance_val.is_undefined() && !has_instance_val.is_null() {
            let fn_ptr = has_instance_val.as_js_object_ptr();
            if !fn_ptr.is_null() {
                let fn_obj = unsafe { &*fn_ptr };
                if fn_obj.is_function() {
                    let result = self.call_function_sync(has_instance_val, rhs_val, &[lhs_val])?;
                    self.regs[rd] = JsValue::bool(to_boolean(result));
                    return Ok(());
                }
            }
        }

        if !lhs_val.is_object() {
            self.regs[rd] = JsValue::bool(false);
            return Ok(());
        }

        let rhs_obj = unsafe { &*rhs_val.as_js_object_ptr() };
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        let ctor_proto = self.resolve_property(rhs_obj, proto_si);

        let ctor_proto_ptr = match ctor_proto {
            Some(v) if v.is_object() => v.as_js_object_ptr(),
            _ => {
                self.regs[rd] = JsValue::bool(false);
                return Ok(());
            }
        };

        let mut proto = unsafe { &*lhs_val.as_js_object_ptr() }.proto();
        let mut depth = 0usize;
        loop {
            if !proto.is_object() {
                self.regs[rd] = JsValue::bool(false);
                break;
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                self.regs[rd] = JsValue::bool(false);
                break;
            }
            depth += 1;
            let proto_ptr = proto.as_js_object_ptr();
            if proto_ptr == ctor_proto_ptr {
                self.regs[rd] = JsValue::bool(true);
                break;
            }
            proto = unsafe { &*proto_ptr }.proto();
        }
        Ok(())
    }

    pub(crate) fn dispatch_in(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("IN rd={}", rd);
        let key_val = self.regs[a];
        let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            return self.raise_type_error("IN right-hand side is not an object");
        }
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(key_val)?;
        let found = self.resolve_property(obj, prop_name_si).is_some();
        self.regs[rd] = JsValue::bool(found);
        Ok(())
    }

    pub(crate) fn dispatch_for_in_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_IN_INIT r{}={:?}", a, self.regs[a]);
        let obj_val = self.regs[a];
        if obj_val.is_null() || obj_val.is_undefined() {
            // null/undefined 枚举不到任何键——是空 for-in，而非 TypeError。
            let keys_vec: bumpalo::collections::Vec<(JsValue, u32)> =
                bumpalo::collections::Vec::new_in(self.epoch.bump());
            let iter = self.epoch.alloc(ForInIter { keys: keys_vec, index: 0 });
            self.iters.push_for_in(iter.cast::<ForInIter<'static>>());
            return Ok(());
        }
        if !obj_val.is_object() {
            // 未支持：其它基本类型的 ToObject 强转尚未实现，在此之前抛 TypeError 是正确行为。
            return self.raise_type_error("for-in right-hand side is not an object");
        }

        let mut keys_vec: bumpalo::collections::Vec<(JsValue, u32)> =
            bumpalo::collections::Vec::new_in(self.epoch.bump());
        let mut seen = std::collections::HashSet::new();
        let mut current = obj_val;

        // 数组把整型下标元素存在 prop_vec，而 prop_vec 不属于 shape 链。
        // 单独枚举它们（ES：数组下标是可枚举字符串键，按升序排在其它自有键之前）。
        // hole（删除标记）跳过——for-in 不枚举数组稀疏空洞。
        if current.is_object() {
            let arr = unsafe { &*current.as_js_object_ptr() };
            if arr.is_array() {
                for i in 0..arr.array_prop_count {
                    if arr.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                        continue;
                    }
                    let is_enum = arr
                        .prop_meta_at(i)
                        .map(|m| m.attributes.enumerable())
                        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                    if is_enum {
                        let idx = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                        keys_vec.push((JsValue::perm_string(self.kernel_core.perm_interner().string_ptr(idx)), idx));
                    }
                }
            }
        }

        let mut depth = 0usize;

        loop {
            if !current.is_object() {
                break;
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                break;
            }
            depth += 1;
            let cur = unsafe { &*current.as_js_object_ptr() };
            let obj_start = keys_vec.len();
            let mut cursor = Some(cur.shape_id());
            while let Some(id) = cursor {
                if id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                    break;
                }
                if let Some(shape) = self.kernel_core.shape_forge().get_shape(id) {
                    // Symbol 键/私有名键不参与 for-in 枚举。
                    if shape.property_name != u32::MAX
                        && !is_symbol_key(shape.property_name)
                        && !is_private_name_key(shape.property_name)
                        && seen.insert(shape.property_name)
                    {
                        let enumerable = self
                            .kernel_core
                            .shape_forge()
                            .lookup_position(cur.shape_id(), shape.property_name)
                            .and_then(|pos| cur.prop_meta_at(pos))
                            .map(|meta| meta.attributes.enumerable())
                            .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                        if enumerable {
                            keys_vec.push((
                                JsValue::perm_string(self.kernel_core.perm_interner().string_ptr(shape.property_name)),
                                shape.property_name,
                            ));
                        }
                    }
                    cursor = shape.parent;
                } else {
                    break;
                }
            }
            // shape 链按叶→根遍历（与插入序相反）；在接上原型前把本对象的切片
            // 翻转为插入序。
            keys_vec[obj_start..].reverse();
            current = cur.proto();
        }

        // ES 枚举序：整型下标键升序，其余按插入序。稳定排序保持非下标键间的插入序。
        keys_vec.sort_by(|(_, a_si), (_, b_si)| {
            match (self.array_index_from_property_key(*a_si), self.array_index_from_property_key(*b_si)) {
                (Some(ai), Some(bi)) => ai.cmp(&bi),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        let iter = self.epoch.alloc(ForInIter { keys: keys_vec, index: 0 });
        self.iters.push_for_in(iter.cast::<ForInIter<'static>>());
        Ok(())
    }

    pub(crate) fn dispatch_for_in_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_IN_NEXT rd={}", rd);
        let iter_ptr = self.iters.last_for_in();
        if iter_ptr.is_null() {
            return Err("FOR_IN_NEXT without active iterator".into());
        }
        let iter = unsafe { &mut *iter_ptr };
        if iter.index < iter.keys.len() {
            self.regs[rd] = iter.keys[iter.index].0;
            iter.index += 1;
        } else {
            self.regs[rd] = JsValue::undefined();
        }
        Ok(())
    }

    pub(crate) fn dispatch_for_in_done(&mut self, rd: usize) {
        vm_trace!("FOR_IN_DONE rd={}", rd);
        let iter_ptr = self.iters.last_for_in();
        if iter_ptr.is_null() {
            self.regs[rd] = JsValue::bool(true);
        } else {
            let iter = unsafe { &*iter_ptr };
            self.regs[rd] = JsValue::bool(iter.index >= iter.keys.len());
        }
    }

    pub(crate) fn dispatch_for_in_cleanup(&mut self) {
        vm_trace!("FOR_IN_CLEANUP");
        self.iters.pop_for_in();
    }

    pub(crate) fn dispatch_for_of_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_INIT r{}={:?}", a, self.regs[a]);
        let iterable = self.regs[a];
        match oxide_builtins::iterator::make_iterator_for_value(self, iterable) {
            Ok(iterator) => {
                self.iters.push_for_of(iterator);
                self.iters.clear_last_for_of_result();
                Ok(())
            }
            Err(err) => {
                self.exception_value = Some(err);
                self.pending_error_kind = Some(self.thrown_error_kind(err));
                self.unwind()
            }
        }
    }

    /// for-await-of 初始化：按 GetAsyncIterator 协议取异步迭代器（缺失 `@@asyncIterator`
    /// 时回退同步迭代器并包 AsyncFromSyncIterator），压入 for-of 迭代器栈。
    pub(crate) fn dispatch_for_await_of_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_INIT r{}={:?}", a, self.regs[a]);
        let iterable = self.regs[a];
        match crate::async_from_sync::make_async_iterator(self, iterable) {
            Ok(iterator) => {
                self.iters.push_for_of(iterator);
                self.iters.clear_last_for_of_result();
                Ok(())
            }
            Err(err) => {
                self.exception_value = Some(err);
                self.pending_error_kind = Some(self.thrown_error_kind(err));
                self.unwind()
            }
        }
    }

    /// for-await-of 步进：调用迭代器 `next()`，把返回的 promise 写入 rd。
    /// 随后的 `AWAIT` 负责挂起等待；next() 抛出时弹出迭代器并透传（与同步 for-of 一致）。
    pub(crate) fn dispatch_for_await_of_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_NEXT rd={}", rd);
        self.last_uncaught_value = None;
        let Some(iterator) = self.iters.last_for_of() else {
            return Err("FOR_AWAIT_OF_NEXT without active iterator".into());
        };
        if !iterator.is_object() {
            return Err("FOR_AWAIT_OF_NEXT iterator is not an object".into());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        let result = match self.call_function_sync(next_fn, iterator, &[]) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        self.regs[rd] = result;
        Ok(())
    }

    /// for-await-of 步进完成检查：读取 `AWAIT` 恢复值（a 槽，即迭代器结果对象）的
    /// `done`，写入 last_for_of_result 并把 `!done` 写 rd（供 JMP_IF_FALSE 分支）。
    pub(crate) fn dispatch_for_await_of_done(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_DONE rd={} r{}={:?}", rd, a, self.regs[a]);
        let result = self.regs[a];
        if !result.is_object() {
            return self.raise_type_error("iterator result is not an object");
        }
        self.iters.set_last_for_of_result(result);
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let done_val = match self.ordinary_get(result_obj, done_si, result) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        self.regs[rd] = JsValue::bool(!to_boolean(done_val));
        Ok(())
    }

    /// for-await-of 收尾：迭代器自然 done 时跳过；否则执行异步 IteratorClose——
    /// 调用 `return()`，其返回的 promise 经 await 挂起，恢复后继续循环后的指令。
    pub(crate) fn dispatch_for_await_of_close(&mut self) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_CLOSE");
        let Some(iterator) = self.iters.pop_for_of() else {
            return Ok(());
        };
        let result = self.iters.last_for_of_result();
        if result.is_object() {
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done_si = self.kernel_core.perm_interner().intern("done").0;
            if let Ok(done_val) = self.ordinary_get(result_obj, done_si, result) {
                if to_boolean(done_val) {
                    return Ok(());
                }
            }
        }
        if !iterator.is_object() {
            return Ok(());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let return_si = self.kernel_core.perm_interner().intern("return").0;
        let return_fn = match self.ordinary_get(iter_obj, return_si, iterator) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        if !return_fn.is_object() {
            return Ok(());
        }
        if !unsafe { &*return_fn.as_js_object_ptr() }.is_function() {
            return Ok(());
        }
        let inner = match self.call_function_sync(return_fn, iterator, &[]) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        if !inner.is_object() {
            return self.raise_type_error("iterator return() result is not an object");
        }
        self.perform_await(inner)
    }

    pub(crate) fn dispatch_for_of_done(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_DONE rd={}", rd);
        let Some(iterator) = self.iters.last_for_of() else {
            return Err("FOR_OF_DONE without active iterator".into());
        };
        if !iterator.is_object() {
            return Err("FOR_OF_DONE iterator is not an object".into());
        }

        self.last_uncaught_value = None;
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        let result = match self.call_function_sync(next_fn, iterator, &[]) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        if !result.is_object() {
            return self.raise_type_error("iterator result is not an object");
        }
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let done_val = match self.ordinary_get(result_obj, done_si, result) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        let done = to_boolean(done_val);
        self.iters.set_last_for_of_result(result);
        self.regs[rd] = JsValue::bool(!done);
        Ok(())
    }

    pub(crate) fn dispatch_for_of_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_NEXT rd={}", rd);
        self.last_uncaught_value = None;
        let result = self.iters.last_for_of_result();
        if !result.is_object() {
            self.regs[rd] = JsValue::undefined();
            return Ok(());
        }
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        self.regs[rd] = match self.ordinary_get(result_obj, value_si, result) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        Ok(())
    }

    /// next()/value 访问抛出：经 unwind 走异常展开，使外围 try/catch 能捕获，并尽可能
    /// 重新抛出原始值。按 ECMA-262，next() 抛出时不会经 return() 关闭迭代器——
    /// 先弹出它，使展开时的 IteratorClose 遍历跳过该迭代器。
    fn throw_for_of_error(&mut self, msg: String) -> Result<(), String> {
        self.iters.pop_for_of();
        let exc = match self.last_uncaught_value.take() {
            Some(v) => v,
            None => oxide_builtins::error::create_error(self, &msg),
        };
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        self.unwind()
    }

    pub(crate) fn dispatch_for_of_close(&mut self) -> Result<(), String> {
        vm_trace!("FOR_OF_CLOSE");
        let Some(iterator) = self.iters.pop_for_of() else {
            return Ok(());
        };
        // 迭代已自然结束（最后一次 next 返回 done:true）时不调 return()；
        // 仅当元素耗尽但迭代器未 done（提前退出）才执行 IteratorClose。
        let result = self.iters.last_for_of_result();
        if result.is_object() {
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done_si = self.kernel_core.perm_interner().intern("done").0;
            if let Ok(done_val) = self.ordinary_get(result_obj, done_si, result) {
                if to_boolean(done_val) {
                    return Ok(());
                }
            }
        }
        // 正常 / break / return 退出：此前无进行中的突然完成，return() 自身的抛出直接传播。
        self.close_for_of_iterator(iterator, false)
    }

    /// 调用 `iterator.return()`（IteratorClose）。
    ///
    /// # 边界与前提
    /// - `suppress_return_error` 为 true 时表示正在展开某个外围突然完成：return() 的
    ///   自身结果被丢弃，并保留在途异常跨调用存活。
    /// - 为 false 时（正常 for-of 退出）return() 的错误向外传播。
    pub(crate) fn close_for_of_iterator(
        &mut self, iterator: JsValue, suppress_return_error: bool,
    ) -> Result<(), String> {
        if !iterator.is_object() {
            return Ok(());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let return_si = self.kernel_core.perm_interner().intern("return").0;
        let return_fn = match self.ordinary_get(iter_obj, return_si, iterator) {
            Ok(v) => v,
            Err(e) => {
                if suppress_return_error {
                    return Ok(());
                }
                return Err(e);
            }
        };
        if return_fn.is_object() {
            let return_obj = unsafe { &*return_fn.as_js_object_ptr() };
            if return_obj.is_function() {
                if suppress_return_error {
                    let saved_exc = self.exception_value;
                    let saved_kind = self.pending_error_kind;
                    let _ = self.call_function_sync(return_fn, iterator, &[]);
                    self.exception_value = saved_exc;
                    self.pending_error_kind = saved_kind;
                } else {
                    let inner = match self.call_function_sync(return_fn, iterator, &[]) {
                        Ok(v) => v,
                        // return() 抛出：恢复原始异常值并展开，使外围 try/catch 可捕获。
                        Err(e) => return self.raise_call_error(&e).map(|_| ()),
                    };
                    // IteratorClose 要求 return() 返回值是对象，否则抛 TypeError。
                    if !inner.is_object() {
                        self.raise_type_error("iterator return() result is not an object")?;
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// 对 `depth` 以上的所有活跃 for-of 迭代器执行 IteratorClose，供 `unwind()` 关闭
    /// 被异常中断的循环。先弹出再关闭，防止重入调用重复关闭，同时跳过 next()-throw
    /// 路径（它已弹出自己的迭代器）。
    pub(crate) fn close_for_of_above(&mut self, depth: usize) {
        while self.iters.for_of_iters.len() > depth {
            let Some(iterator) = self.iters.pop_for_of() else {
                break;
            };
            let _ = self.close_for_of_iterator(iterator, true);
        }
    }

    pub(crate) fn dispatch_rest_object(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("REST_OBJECT rd={}", rd);
        // ext 字（excluded 常量下标）在任何路径都先消费，防止 ToObject 早返回后
        // pc 错位把 ext 字当下一条指令解码（曾误读成 UNSPILL slot）。
        let excluded_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let src = self.regs[a];
        if !src.is_object() {
            // ToObject：null/undefined 抛；number/boolean/symbol 无自有属性 → 空对象；
            // string 的索引字符（UTF-16 code unit）是可枚举自有属性。
            if src.is_null() || src.is_undefined() {
                return self.raise_type_error("Cannot destructure property of null/undefined");
            }
            let proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
            let rest_ptr = self.alloc_object(JsObject::new_empty(
                oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
                JsValue::from_js_object(proto_ptr),
            ));
            if src.is_string() {
                let code_units: Vec<u16> = unsafe { (*src.as_string_ptr()).data.encode_utf16().collect() };
                for (i, unit) in code_units.iter().enumerate() {
                    let s = char::from_u32(*unit as u32).map(|c| c.to_string()).unwrap_or_default();
                    let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                    let ch_val = self.new_string(&s);
                    let rest = unsafe { &mut *rest_ptr };
                    self.set_or_create_prop_value(rest, si, ch_val);
                }
            }
            self.regs[rd] = JsValue::from_js_object(rest_ptr);
            return Ok(());
        }
        let excluded_const = self
            .immutables()
            .get(excluded_idx)
            .and_then(|v| {
                if v.is_string() {
                    // SAFETY: v 是字符串常量值。
                    Some(unsafe { (*v.as_string_ptr()).data.clone() })
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let mut excluded: std::collections::HashSet<u32> = excluded_const
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|s| self.kernel_core.perm_interner().intern(s).0)
            .collect();
        // 运行时 excluded：b 槽数组（computed key 求值结果）的元素 ToPropertyKey 后排除。
        if b != 0 {
            let arr_val = self.regs[b];
            if arr_val.is_object() {
                let arr_obj = unsafe { &*arr_val.as_js_object_ptr() };
                if arr_obj.is_array() {
                    for i in 0..arr_obj.array_prop_count {
                        let v = arr_obj.get_prop_at(i);
                        let si = self.property_key_si(v)?;
                        excluded.insert(si);
                    }
                }
            }
        }

        let src_obj = unsafe { &*src.as_js_object_ptr() };

        // 收集-提交：先取齐 (键, 值)，取值阶段触发 getter（可能抛异常），再统一写
        // 目标对象，避免提交写与取值互相交错。只复制可枚举自有属性（CopyDataProperties）。
        let mut assignments: Vec<(u32, JsValue)> = Vec::new();

        // 字符串包装对象：索引字符是可枚举自有属性（ToObject("str") 的 0..len-1）。
        if src_obj.type_tag == JsObject::OBJ_TYPE_STRING_OBJ {
            let raw = src_obj.get_prop_at(0);
            let s = unsafe { (*raw.as_string_ptr()).data.clone() };
            let code_units: Vec<u16> = s.encode_utf16().collect();
            for (i, unit) in code_units.iter().enumerate() {
                let ch = char::from_u32(*unit as u32).map(|c| c.to_string()).unwrap_or_default();
                let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                assignments.push((si, self.new_string(&ch)));
            }
        }

        // 数组元素区：整数下标可枚举元素（hole 跳过）。
        if src_obj.is_array() {
            for i in 0..src_obj.array_prop_count {
                if src_obj.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                    continue;
                }
                let enumerable = src_obj
                    .prop_meta_at(i)
                    .map(|m| m.attributes.enumerable())
                    .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                if enumerable {
                    let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                    let val = match self.ordinary_get(src_obj, si, src) {
                        Ok(v) => v,
                        Err(e) => return self.raise_call_error(&e).map(|_| ()),
                    };
                    assignments.push((si, val));
                }
            }
        }

        // 命名属性：shape 链（walk_own_keys 规范顺序），仅可枚举，跳过 pattern 已绑定的键。
        let keys = oxide_builtins::object::walk_own_keys(self, src_obj);
        for (si, pos) in keys {
            let store = if src_obj.is_array() { src_obj.array_prop_count + pos } else { pos };
            let enumerable = src_obj
                .prop_meta_at(store)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
            if !enumerable {
                continue;
            }
            if excluded.contains(&si) {
                continue;
            }
            let val = match self.ordinary_get(src_obj, si, src) {
                Ok(v) => v,
                Err(e) => return self.raise_call_error(&e).map(|_| ()),
            };
            assignments.push((si, val));
        }

        // 提交到 rest 对象。
        let proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let rest_ptr = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        let rest = unsafe { &mut *rest_ptr };
        for (si, val) in assignments {
            let promoted = self.promote_if_needed_for_write_ptr(rest_ptr, val);
            self.set_or_create_prop_value(rest, si, promoted);
        }
        self.regs[rd] = JsValue::from_js_object(rest_ptr);
        Ok(())
    }

    /// SPREAD_OBJECT：对象字面量 `...` 展开，把源的可枚举自有属性写入目标对象（原地改）。
    ///
    /// 语义 = CopyDataProperties 的"非 null/undefined 源"分支：`{...null}` / `{...undefined}`
    /// 合法（产出空展开），与 REST_OBJECT 对 null/undefined 抛 TypeError 不同。
    ///
    /// # 步骤
    /// 1. null/undefined 源直接返回（空展开）
    /// 2. 字符串源按 UTF-16 code unit 下标复制为可枚举索引属性
    /// 3. 对象源先收集可枚举自有属性（数组元素区 + shape 链），取值经 ordinary_get
    ///    触发访问器 getter，再统一写入目标
    ///
    /// # 边界与前提
    /// - 其它原始值（number/boolean/symbol）无自有可枚举属性 → 空展开
    /// - 目标已有同名属性被覆盖（后定义/后展开者胜）
    ///
    /// # 副作用
    /// - 原地修改 rd 指向的对象；取值可能触发 getter 并抛异常（异常传播）
    pub(crate) fn dispatch_spread_object(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("SPREAD_OBJECT rd={}", rd);
        let target_val = self.regs[rd];
        let src = self.regs[a];

        // null/undefined 源合法：展开为空，直接返回。
        if src.is_null() || src.is_undefined() {
            return Ok(());
        }

        // 字符串源：按索引复制字符（可枚举索引属性）。
        if src.is_string() {
            let target = unsafe { &mut *target_val.as_js_object_ptr() };
            let code_units: Vec<u16> = unsafe { (*src.as_string_ptr()).data.encode_utf16().collect() };
            for (i, unit) in code_units.iter().enumerate() {
                let s = char::from_u32(*unit as u32).map(|c| c.to_string()).unwrap_or_default();
                let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                let ch_val = self.new_string(&s);
                self.set_or_create_prop_value(target, si, ch_val);
            }
            return Ok(());
        }

        // 其它原始值无自有可枚举属性 → 空展开。
        if !src.is_object() {
            return Ok(());
        }

        let src_obj = unsafe { &*src.as_js_object_ptr() };

        // 收集-提交：先取齐 (键, 值)，取值阶段触发 getter（可能抛异常），
        // 再统一写目标，避免提交写与取值互相交错。
        let mut assignments: Vec<(u32, JsValue)> = Vec::new();

        // 数组元素区：整数下标可枚举元素（hole 跳过）。
        if src_obj.is_array() {
            for i in 0..src_obj.array_prop_count {
                if src_obj.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                    continue;
                }
                let enumerable = src_obj
                    .prop_meta_at(i)
                    .map(|m| m.attributes.enumerable())
                    .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                if enumerable {
                    let si = self.kernel_core.perm_interner().intern(&i.to_string()).0;
                    let val = self.ordinary_get(src_obj, si, src)?;
                    assignments.push((si, val));
                }
            }
        }

        // 命名属性：shape 链（walk_own_keys 规范顺序），仅可枚举。
        let keys = oxide_builtins::object::walk_own_keys(self, src_obj);
        for (si, pos) in keys {
            let store = if src_obj.is_array() { src_obj.array_prop_count + pos } else { pos };
            let enumerable = src_obj
                .prop_meta_at(store)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
            if !enumerable {
                continue;
            }
            let val = self.ordinary_get(src_obj, si, src)?;
            assignments.push((si, val));
        }

        // 提交：覆盖目标已有同名属性（从左到右求值，后展开者胜）。
        let target = unsafe { &mut *target_val.as_js_object_ptr() };
        for (si, val) in assignments {
            let promoted = self.promote_if_needed_for_write_ptr(target, val);
            self.set_or_create_prop_value(target, si, promoted);
        }
        Ok(())
    }
}
