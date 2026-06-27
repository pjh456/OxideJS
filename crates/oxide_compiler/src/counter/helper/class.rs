use super::super::*;

impl Compiler {
    pub(in crate::counter) fn count_class(&self, class: &oxide_parser::Class, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // ctor_reg
        ctx.alloc_reg(); // proto_reg
        if let Some(super_class) = &class.super_class {
            self.count_expression(super_class, ctx);
        }
        ctx.count_instr(); // LOAD_CONST ctor
        ctx.count_instr(); // NEW_OBJECT proto

        if class.super_class.is_some() {
            ctx.alloc_reg(); // parent prototype key
            ctx.count_instr(); // LOAD_CONST
            ctx.alloc_reg(); // parent prototype value
            ctx.count_instr(); // GET_PROP
            ctx.alloc_reg(); // __proto__ key
            ctx.count_instr(); // LOAD_CONST
            ctx.count_instr(); // SET_PROP proto.__proto__
            ctx.count_instr(); // SET_PROP ctor.__proto__
        }

        ctx.count_load_const(); // constructor key
        ctx.count_instr(); // SET_PROP proto.constructor = ctor

        ctx.count_load_const(); // prototype key
        ctx.count_instr(); // SET_PROP ctor.prototype = proto

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if matches!(method.kind, MethodDefinitionKind::Constructor) {
                        continue;
                    }
                    if matches!(method.key, oxide_parser::PropertyKey::PrivateIdentifier(_)) {
                        // Mirror emit_private_method_init for static AND non-static private
                        // methods: private-id LOAD_CONST + CREATE_CLOSURE + SET_HOME_OBJECT +
                        // INIT_PRIVATE.
                        ctx.count_load_const(); // private id reg
                        ctx.count_load_const(); // method closure (CREATE_CLOSURE)
                        ctx.count_instr(); // SET_HOME_OBJECT
                        ctx.count_instr(); // INIT_PRIVATE
                        continue;
                    }
                    // Computed method key emits the full key expression; a plain key is one
                    // LOAD_CONST. Mirror emit_class_key_reg via the shared count_class_key.
                    self.count_class_key(&method.key, method.computed, ctx);
                    ctx.count_load_const(); // method closure (CREATE_CLOSURE)
                    ctx.count_instr(); // SET_HOME_OBJECT
                    match method.kind {
                        MethodDefinitionKind::Method => {
                            ctx.count_instr(); // SET_PROP
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            ctx.count_load_const(); // undefined placeholder
                            ctx.count_define_accessor(); // DEFINE_ACCESSOR + ext
                        }
                        MethodDefinitionKind::Constructor => {}
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        // Computed static field key emits the full key expression; mirror
                        // emit_public_field_init via the shared count_class_key.
                        self.count_class_key(&prop.key, prop.computed, ctx);
                        if let Some(value) = &prop.value {
                            self.count_expression(value, ctx);
                        } else {
                            ctx.count_load_const();
                        }
                        ctx.count_instr(); // SET_PROP
                    }
                }
                ClassElement::StaticBlock(block) => {
                    for stmt in &block.body {
                        self.count_statement(stmt, ctx);
                    }
                }
                ClassElement::AccessorProperty(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }
    }
}
