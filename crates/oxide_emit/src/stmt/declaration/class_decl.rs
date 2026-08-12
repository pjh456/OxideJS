//! 类声明语句 emit：`emit_class_declaration_statement` 声明类名并初始化类对象。

use crate::{CompileCtx, Emitter};
use oxide_parser::{Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_class_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let Statement::ClassDeclaration(class) = stmt else {
            return Err("ClassDeclaration without name".into());
        };
        let name = class
            .id
            .as_ref()
            .map(|id| id.name.to_string())
            .ok_or_else(|| "ClassDeclaration without name".to_string())?;
        let var_reg = match ctx.scopes.symbols.consume_predeclared_slot(&name) {
            Some(reg) => reg,
            None => {
                let reg = ctx.alloc_reg();
                ctx.declare(&name, reg, VariableDeclarationKind::Let, false)?;
                reg
            }
        };
        // 类名绑定在 emit_class_with_binding 内于 extends 求值后初始化：
        // 先声明未初始化绑定（TDZ），extends 引用类名按规范抛 ReferenceError。
        self.emit_class_with_binding(class, ctx, Some(var_reg))?;
        Ok(None)
    }
}
