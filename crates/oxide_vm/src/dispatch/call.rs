use crate::native::NativeFn;
use crate::vm::Vm;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

impl Vm {
    #[inline(always)]
    pub(crate) fn build_native_args(
        first_arg_reg: u8,
        arg_count: usize,
        this_reg: u8,
    ) -> ([u8; 257], usize) {
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
        &mut self,
        obj: &JsObject,
        callee: JsValue,
        this_reg: u8,
        first_arg_reg: u8,
        arg_count: usize,
    ) -> Result<(), String> {
        let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, this_reg);
        let args_slice = &args_buf[..len];

        let func: NativeFn = unsafe { std::mem::transmute(obj.native_fn().unwrap()) };
        self.regs[254] = callee;
        match func(self, args_slice) {
            Ok(val) => {
                self.regs[0] = val;
                Ok(())
            }
            Err(err_val) => {
                let msg = if err_val.is_string() {
                    self.kernel()
                        .string_forge()
                        .lookup(err_val.as_string_index())
                        .unwrap_or_else(|| format!("{err_val}"))
                } else {
                    format!("{err_val}")
                };
                let error = crate::builtins::error::create_error(self, &msg);
                self.exception_value = Some(error);
                self.pending_error_kind = Some("Error");
                self.unwind()
            }
        }
    }
}
