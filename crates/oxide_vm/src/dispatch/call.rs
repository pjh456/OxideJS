use crate::native::{NativeFn, NativeResult};
use crate::vm::{native_fn_ptr_to_fn, FrameContinuation, Vm};
use oxide_kernel::{builtins_debug, builtins_trace};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

impl Vm {
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

        // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
        // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
        let func: NativeFn = unsafe { native_fn_ptr_to_fn(obj.native_fn().unwrap()) };
        self.regs[254] = callee;
        self.native_call_depth += 1;
        match func(self, args_slice) {
            NativeResult::Ok(val) => {
                self.native_call_depth -= 1;
                builtins_trace!("native_call ok depth={}", self.native_call_depth);
                self.regs[0] = val;
                Ok(())
            }
            NativeResult::Err(err_val) => {
                self.native_call_depth -= 1;
                let (error, kind) = if err_val.is_object() {
                    (err_val, self.thrown_error_kind(err_val))
                } else {
                    let msg = if err_val.is_string() {
                        self.kernel_core
                            .string_forge()
                            .lookup(err_val.as_string_index())
                            .unwrap_or_else(|| format!("{err_val}"))
                    } else {
                        format!("{err_val}")
                    };
                    builtins_debug!("native_call err={}", msg);
                    (crate::builtins::error::create_error(self, &msg), "Error")
                };
                self.exception_value = Some(error);
                self.pending_error_kind = Some(kind);
                self.unwind()
            }
            NativeResult::TailCall { callee, this, args } => {
                self.native_call_depth -= 1;
                if callee.is_object() {
                    let obj = unsafe { &*callee.as_js_object_ptr() };
                    if obj.native_fn().is_some() {
                        let result = self.call_function_sync(callee, this, &args)?;
                        self.regs[0] = result;
                        return Ok(());
                    }
                }
                self.push_bytecode_frame(callee, this, &args, None, None, JsValue::undefined(), FrameContinuation::None)
            }
        }
    }
}
