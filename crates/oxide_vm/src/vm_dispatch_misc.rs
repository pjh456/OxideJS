use std::sync::Arc;

use crate::native::NativeFn;
use crate::vm::{native_fn_ptr_to_fn, CallFrame, ForInIter, FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::vm_trace;
use oxide_runtime_api::{to_boolean, NativeResult};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::is_private_name_key;
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
            let sub_idx = ctor_obj.sub_module_index() as usize - 1;
            if sub_idx >= self.sub_modules.len() {
                return Err(format!(
                    "NEW_EXPRESSION: sub_module_index {} out of bounds (max {})",
                    sub_idx,
                    self.sub_modules.len()
                ));
            }

            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err(self.error_message_text("RangeError", "Maximum call stack size exceeded"));
            }

            let new_obj_val = JsValue::object(new_obj as *mut u8);
            let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
            let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
            let sub_n_registers = self.sub_modules[sub_idx].n_registers;
            let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
            let caller_reg_limit = self.active_reg_limit.max(1);
            let saved_reg_offset = self.save_stack.len() as u32;
            self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
            let saved_this = self.regs[254];
            let saved_new_target = self.regs[255];

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
            self.activate_immutables(sub_idx + 1, &subs[sub_idx].constants);

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

    pub(crate) fn dispatch_template_str(&mut self, rd: usize) {
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
                let reg = (seg & 0x7F) as u8;
                let val = self.regs[reg as usize];
                let s = if val.is_string() {
                    // SAFETY: val 是字符串值。
                    unsafe { (*val.as_string_ptr()).data.clone() }
                } else {
                    format!("{}", val)
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
        let has_instance_si = self.property_key_si(has_instance_key);

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
        let prop_name_si = self.property_key_si(key_val);
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
        if current.is_object() {
            let arr = unsafe { &*current.as_js_object_ptr() };
            if arr.is_array() {
                for i in 0..arr.prop_vec_len() {
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
                    if shape.property_name != u32::MAX
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
                    let _ = self.call_function_sync(return_fn, iterator, &[])?;
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

    pub(crate) fn dispatch_rest_object(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("REST_OBJECT rd={}", rd);
        let src = self.regs[a];
        if !src.is_object() {
            return self.raise_type_error("Cannot destructure property of null/undefined");
        }
        let excluded_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let excluded = self
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
        let excluded: std::collections::HashSet<&str> = excluded.split('\0').filter(|s| !s.is_empty()).collect();

        let proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let rest_ptr = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        let src_obj = unsafe { &*src.as_js_object_ptr() };
        let mut cursor = Some(src_obj.shape_id());
        while let Some(shape_id) = cursor {
            if shape_id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                break;
            }
            let Some(shape) = self.kernel_core.shape_forge().get_shape(shape_id) else {
                break;
            };
            if shape.property_name != u32::MAX && !is_private_name_key(shape.property_name) {
                if let Some(name) = self.kernel_core.perm_interner().lookup(shape.property_name) {
                    if !excluded.contains(name) {
                        if let Some(pos) = self
                            .kernel_core
                            .shape_forge()
                            .lookup_position(src_obj.shape_id(), shape.property_name)
                        {
                            let val = src_obj.get_prop_at(pos);
                            let rest = unsafe { &mut *rest_ptr };
                            let val = self.promote_if_needed_for_write_ptr(rest_ptr, val);
                            self.set_or_create_prop_value(rest, shape.property_name, val);
                        }
                    }
                }
            }
            cursor = shape.parent;
        }
        self.regs[rd] = JsValue::from_js_object(rest_ptr);
        Ok(())
    }
}
