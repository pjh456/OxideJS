use oxide_compiler::opcode::OpCode;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::vm::Vm;

impl Vm {
    pub(crate) fn dispatch_member_op(
        &mut self,
        op: OpCode,
        rd: usize,
        a: usize,
        b: usize,
    ) -> Result<(), String> {
        match op {
            OpCode::MEMBER_INC => self.dispatch_member_inc(rd, a, b),
            OpCode::MEMBER_DEC => self.dispatch_member_dec(rd, a, b),
            OpCode::DYN_MEMBER_INC => self.dispatch_dyn_member_inc(rd, a, b),
            OpCode::DYN_MEMBER_DEC => self.dispatch_dyn_member_dec(rd, a, b),
            OpCode::COMPOUND_MEMBER_ADD => self.dispatch_compound_member_add(rd, a, b),
            OpCode::COMPOUND_MEMBER_SUB => self.dispatch_compound_member_numeric(
                rd,
                a,
                b,
                "COMPOUND_MEMBER_SUB on non-object",
                |l, r| l - r,
            ),
            OpCode::COMPOUND_MEMBER_MUL => self.dispatch_compound_member_numeric(
                rd,
                a,
                b,
                "COMPOUND_MEMBER_MUL on non-object",
                |l, r| l * r,
            ),
            OpCode::COMPOUND_MEMBER_DIV => self.dispatch_compound_member_numeric(
                rd,
                a,
                b,
                "COMPOUND_MEMBER_DIV on non-object",
                |l, r| l / r,
            ),
            OpCode::COMPOUND_MEMBER_MOD => self.dispatch_compound_member_numeric(
                rd,
                a,
                b,
                "COMPOUND_MEMBER_MOD on non-object",
                |l, r| l % r,
            ),
            OpCode::COMPOUND_MEMBER_EXP => self.dispatch_compound_member_numeric(
                rd,
                a,
                b,
                "COMPOUND_MEMBER_EXP on non-object",
                |l, r| l.powf(r),
            ),
            _ => unreachable!("non-member opcode passed to dispatch_member_op"),
        }
    }

    fn member_target(
        &mut self,
        rd: usize,
        b: usize,
        error_msg: &str,
    ) -> Result<(*mut JsObject, u32), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error(error_msg)?;
            return Err(String::new());
        }
        let prop_name_si = self.property_key_si(self.regs[b]);
        Ok((obj_ptr, prop_name_si))
    }

    fn dispatch_member_inc(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Ok((obj_ptr, prop_name_si)) = self.member_target(rd, b, "MEMBER_INC on non-object")
        else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_val = self.read_member_prop(obj, prop_name_si);
        let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
        let new_val = JsValue::float(n + 1.0);
        self.set_member_prop(obj, prop_name_si, new_val)?;
        self.regs[a] = new_val;
        Ok(())
    }

    fn dispatch_member_dec(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Ok((obj_ptr, prop_name_si)) = self.member_target(rd, b, "MEMBER_DEC on non-object")
        else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_val = self.read_member_prop(obj, prop_name_si);
        let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
        let new_val = JsValue::float(n - 1.0);
        self.set_member_prop(obj, prop_name_si, new_val)?;
        self.regs[a] = new_val;
        Ok(())
    }

    fn dispatch_dyn_member_inc(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("DYN_MEMBER_INC on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        let prop_val = self
            .resolve_property(obj, prop_name_si)
            .unwrap_or(JsValue::undefined());
        let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
        let new_val = JsValue::float(n + 1.0);
        self.set_or_create_prop_value(obj, prop_name_si, new_val);
        self.regs[b] = new_val;
        Ok(())
    }

    fn dispatch_dyn_member_dec(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("DYN_MEMBER_DEC on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        let prop_val = self
            .resolve_property(obj, prop_name_si)
            .unwrap_or(JsValue::undefined());
        let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
        let new_val = JsValue::float(n - 1.0);
        self.set_or_create_prop_value(obj, prop_name_si, new_val);
        self.regs[b] = new_val;
        Ok(())
    }

    fn dispatch_compound_member_add(
        &mut self,
        rd: usize,
        a: usize,
        b: usize,
    ) -> Result<(), String> {
        let Ok((obj_ptr, prop_name_si)) =
            self.member_target(rd, b, "COMPOUND_MEMBER_ADD on non-object")
        else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_val = self.read_member_prop(obj, prop_name_si);
        let rhs = self.regs[a];
        let new_val = if prop_val.is_string() || rhs.is_string() {
            let ls = coercion::to_string(self.kernel.string_forge().as_ref(), prop_val);
            let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
            self.intern(&format!("{ls}{rs}"))
        } else {
            let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
            let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
            JsValue::float(ln + rn)
        };
        self.set_member_prop(obj, prop_name_si, new_val)?;
        self.regs[a] = new_val;
        Ok(())
    }

    fn dispatch_compound_member_numeric<F>(
        &mut self,
        rd: usize,
        a: usize,
        b: usize,
        error_msg: &str,
        op: F,
    ) -> Result<(), String>
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let Ok((obj_ptr, prop_name_si)) = self.member_target(rd, b, error_msg) else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_val = self.read_member_prop(obj, prop_name_si);
        let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
        let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
        let new_val = JsValue::float(op(ln, rn));
        self.regs[a] = new_val;
        self.set_member_prop(obj, prop_name_si, new_val)?;
        Ok(())
    }
}
