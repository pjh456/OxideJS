use crate::vm::Vm;
use crate::{vm_error, vm_trace};
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
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
            // const guard: check if already initialized
            if !self.regs[rd].is_undefined() {
                self.raise_error_kind("TypeError", "Assignment to constant variable")?;
                return Ok(true);
            }
        }
        self.regs[rd] = self.regs[a];
        Ok(false)
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
