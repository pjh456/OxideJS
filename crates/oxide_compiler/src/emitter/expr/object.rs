use super::*;

impl Compiler {
    fn emit_object_expression(&self, obj: &oxide_parser::ObjectExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        let obj_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_OBJECT, obj_reg, 0, 0));
        let prop_checkpoint = ctx.reg_checkpoint();
        for prop in &obj.properties {
            let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop else {
                return Err("spread properties not yet supported".into());
            };
            let prop_name = match &p.key {
                oxide_parser::PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                oxide_parser::PropertyKey::StringLiteral(s) => s.value.to_string(),
                _ => return Err("unsupported object property key type".into()),
            };
            match p.kind {
                PropertyKind::Get | PropertyKind::Set => {
                    let accessor_reg = self.emit_expression(&p.value, ctx)?;
                    if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                        sub_mod.function_name = Some(prop_name.to_string());
                    }
                    let undef_reg = self.emit_undefined(ctx);
                    let (get_reg, set_reg) = if p.kind == PropertyKind::Get {
                        (accessor_reg, undef_reg)
                    } else {
                        (undef_reg, accessor_reg)
                    };
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, obj_reg, get_reg, set_reg));
                    ctx.emit(idx as u32);
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
                _ => {
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    let val_reg = self.emit_expression(&p.value, ctx)?;
                    // Name inference: if property value is an arrow function,
                    // set the compiled sub_module's function_name to the property key.
                    if matches!(&p.value, Expression::ArrowFunctionExpression(_)) {
                        if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                    }
                    ctx.emit(opcode::encode(OpCode::SET_PROP, obj_reg, val_reg, key_reg));
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
            }
        }
        Ok(obj_reg)
    }

    fn emit_array_expression(&self, arr: &oxide_parser::ArrayExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        let arr_reg = ctx.alloc_reg();
        let n = arr.elements.len() as u16;
        ctx.emit(opcode::encode(OpCode::NEW_ARRAY, arr_reg, (n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8));
        let elem_checkpoint = ctx.reg_checkpoint();
        for (i, elem) in arr.elements.iter().enumerate() {
            let Some(e) = elem.as_expression() else {
                return Err("spread not supported".into());
            };
            let val_reg = self.emit_expression(e, ctx)?;
            let idx_reg = ctx.alloc_reg();
            let idx = ctx.add_constant(Constant::Int(i as i32));
            ctx.emit_load_const(idx_reg, idx);
            ctx.emit(opcode::encode(OpCode::SET_ELEM, arr_reg, idx_reg, val_reg));
            ctx.restore_reg_checkpoint(elem_checkpoint);
        }
        Ok(arr_reg)
    }

    pub(in crate::emitter) fn emit_object_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::ObjectExpression(obj) => self.emit_object_expression(obj, ctx),
            Expression::ArrayExpression(arr) => self.emit_array_expression(arr, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
