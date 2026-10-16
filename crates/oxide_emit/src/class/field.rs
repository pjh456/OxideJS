//! 类静态字段/元素 emit：`emit_class_static_elements`。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ClassElement, PropertyKey};

impl Emitter {
    /// 按源码顺序发静态字段初始化与静态块。
    /// JS 类语义要求两者交织执行（如 `static x = 1; static { this.y = 1 }
    /// static z = this.y + 1` 必须按此顺序），若全部字段先于静态块发出，
    /// `z` 会在 `y` 赋值前读到它。
    pub(crate) fn emit_class_static_elements(
        &self, elements: &[ClassElement], ctor_reg: u32, key_slots: &[Option<u8>], ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let saved_static_this = ctx.static_block_this_reg;
        // ctor_reg 是 vreg 号（RegAlloc 期压缩到物理域），直接存 u32，不截断。
        ctx.static_block_this_reg = Some(ctor_reg);
        for (element, &key_slot) in elements.iter().zip(key_slots) {
            match element {
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                            self.emit_private_field_init(
                                Operand::Reg(ctor_reg),
                                private.name.as_str(),
                                prop.value.as_ref(),
                                ctx,
                            )?;
                        } else {
                            // 静态字段在类定义期求值：computed key 从类定义期数组取，其余用常量。
                            let key_reg = if prop.computed {
                                self.emit_class_array_key_reg(key_slot.unwrap_or(0), ctx)?
                            } else {
                                self.emit_class_key_reg(&prop.key, ctx)?
                            };
                            let value_reg = self.emit_field_value(prop.value.as_ref(), ctx)?;
                            ctx.inst(Inst::define_prop(
                                Operand::Reg(ctor_reg),
                                Operand::Reg(value_reg),
                                Operand::Reg(key_reg),
                            ));
                        }
                    }
                }
                ClassElement::StaticBlock(block) => {
                    ctx.push_scope();
                    for stmt in &block.body {
                        self.emit_statement(stmt, ctx)?;
                    }
                    ctx.pop_scope();
                }
                _ => {}
            }
        }
        ctx.static_block_this_reg = saved_static_this;
        Ok(())
    }
}
