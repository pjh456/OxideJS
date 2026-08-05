use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_parser::PropertyKey;

impl Compiler {
    fn private_name_id(&self, name: &str, ctx: &CompileCtx) -> Result<u32, String> {
        ctx.scopes
            .private_name_map
            .iter()
            .find_map(|(n, id)| (n == name).then_some(*id))
            .ok_or_else(|| format!("private name #{name} is not defined"))
    }

    pub(crate) fn emit_private_id_reg(&self, name: &str, ctx: &mut CompileCtx) -> Result<u8, String> {
        let id = self.private_name_id(name, ctx)?;
        let idx = ctx.add_constant(Constant::Int(id as i32));
        let reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(reg as u32), idx));
        Ok(reg)
    }

    pub(crate) fn emit_class_key_reg(
        &self, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        if !computed {
            let name = self.class_property_name(key)?;
            let idx = ctx.add_constant(Constant::String(name));
            let reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(reg as u32), idx));
            return Ok(reg);
        }

        if matches!(key, PropertyKey::PrivateIdentifier(_)) {
            return Err("private class elements not yet supported".into());
        }
        self.emit_expression(key.to_expression(), ctx)
    }

    pub(crate) fn static_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            _ => Err("computed destructuring keys not yet supported".into()),
        }
    }

    pub(crate) fn class_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            PropertyKey::PrivateIdentifier(_) => Err("private class elements not yet supported".into()),
            _ => Err("unsupported class property key type".into()),
        }
    }

    pub(crate) fn emit_undefined(&self, ctx: &mut CompileCtx) -> u8 {
        let idx = ctx.add_constant(Constant::Undefined);
        let reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(reg as u32), idx));
        reg
    }
}
