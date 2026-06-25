use crate::vm::Vm;

impl Vm {
    #[inline(always)]
    pub(crate) fn dispatch_load_const(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let idx = (instr >> 16) as usize;
        let imm = self.immutables();
        if idx < imm.len() {
            self.regs[rd] = imm[idx];
            Ok(())
        } else {
            Err(format!("constant index {idx} out of bounds"))
        }
    }

    #[inline(always)]
    pub(crate) fn dispatch_typeof(&mut self, rd: usize, a: usize) {
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
}
