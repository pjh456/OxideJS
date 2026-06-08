use std::collections::HashMap;

    use oxide_parser::VariableDeclarationKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    FunctionScope,
    BlockScope,
}

pub(crate) struct Scope {
    pub(crate) bindings: HashMap<String, Binding>,
    pub(crate) kind: ScopeKind,
}

pub(crate) struct Binding {
    pub(crate) reg: u8,
    pub(crate) initialized: bool,
    pub(crate) is_const: bool,
}

pub struct SymbolTable {
    pub(crate) scopes: Vec<Scope>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope {
                bindings: HashMap::new(),
                kind: ScopeKind::FunctionScope,
            }],
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            kind: ScopeKind::BlockScope,
        });
    }

    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            kind,
        });
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn find_var_target_scope(&self) -> usize {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.kind == ScopeKind::FunctionScope {
                return i;
            }
        }
        0
    }

    pub fn declare(
        &mut self,
        name: &str,
        reg: u8,
        kind: VariableDeclarationKind,
        is_const: bool,
    ) -> Result<(), String> {
        let target_idx = if matches!(kind, VariableDeclarationKind::Var) {
            self.find_var_target_scope()
        } else {
            self.scopes.len() - 1
        };

        let target = &mut self.scopes[target_idx];
        if target.bindings.contains_key(name) {
            return Err(format!("Identifier '{name}' has already been declared"));
        }
        target.bindings.insert(
            name.to_string(),
            Binding {
                reg,
                initialized: false,
                is_const: matches!(kind, VariableDeclarationKind::Const) || is_const,
            },
        );
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Result<u8, String> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.bindings.get(name) {
                if b.initialized {
                    return Ok(b.reg);
                }
                return Err(format!("Cannot access '{name}' before initialization"));
            }
        }
        Err(format!("Identifier '{name}' is not defined"))
    }

    pub fn lookup_or_global(&mut self, name: &str, reg_for_new: u8) -> u8 {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.bindings.get(name) {
                return b.reg;
            }
        }
        self.scopes[0].bindings.insert(
            name.to_string(),
            Binding {
                reg: reg_for_new,
                initialized: true,
                is_const: false,
            },
        );
        reg_for_new
    }

    pub fn lookup_is_const(&self, name: &str) -> bool {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.bindings.get(name) {
                if b.initialized {
                    return b.is_const;
                }
                return false;
            }
        }
        false
    }

    pub fn declare_initialized(
        &mut self,
        name: &str,
        reg: u8,
        kind: VariableDeclarationKind,
        is_const: bool,
    ) -> Result<(), String> {
        let target_idx = if matches!(kind, VariableDeclarationKind::Var) {
            self.find_var_target_scope()
        } else {
            self.scopes.len() - 1
        };

        let target = &mut self.scopes[target_idx];
        if let Some(b) = target.bindings.get_mut(name) {
            b.initialized = true;
            return Ok(());
        }
        target.bindings.insert(
            name.to_string(),
            Binding {
                reg,
                initialized: true,
                is_const: matches!(kind, VariableDeclarationKind::Const) || is_const,
            },
        );
        Ok(())
    }

    pub fn pre_register_global(&mut self, name: &str, reg: u8) {
        self.scopes[0]
            .bindings
            .entry(name.to_string())
            .or_insert(Binding {
                reg,
                initialized: true,
                is_const: false,
            });
    }

    pub fn init_var(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(b) = scope.bindings.get_mut(name) {
                b.initialized = true;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolTable;
use oxide_parser::VariableDeclarationKind;

    fn v() -> VariableDeclarationKind {
        VariableDeclarationKind::Var
    }
    fn l() -> VariableDeclarationKind {
        VariableDeclarationKind::Let
    }
    fn c() -> VariableDeclarationKind {
        VariableDeclarationKind::Const
    }

    #[test]
    fn global_scope_exists() {
        let st = SymbolTable::new();
        assert!(st.lookup("x").is_err());
    }

    #[test]
    fn declare_and_lookup() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, l(), false).unwrap();
        st.init_var("x");
        assert_eq!(st.lookup("x").unwrap(), 0);
    }

    #[test]
    fn tdz_error() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, l(), false).unwrap();
        assert!(st
            .lookup("x")
            .unwrap_err()
            .contains("before initialization"));
    }

    #[test]
    fn duplicate_declaration_error() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, l(), false).unwrap();
        assert!(st.declare("x", 1, l(), false).is_err());
    }

    #[test]
    fn nested_scopes_shadow() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, l(), false).unwrap();
        st.init_var("x");
        st.push_scope();
        st.declare("x", 1, l(), false).unwrap();
        st.init_var("x");
        assert_eq!(st.lookup("x").unwrap(), 1);
        st.pop_scope();
        assert_eq!(st.lookup("x").unwrap(), 0);
    }

    #[test]
    fn lookup_or_global_auto_create() {
        let mut st = SymbolTable::new();
        let reg = st.lookup_or_global("x", 5);
        assert_eq!(reg, 5);
        assert_eq!(st.lookup("x").unwrap(), 5);
    }

    #[test]
    fn lookup_or_global_existing() {
        let mut st = SymbolTable::new();
        st.declare("x", 3, l(), false).unwrap();
        st.init_var("x");
        let reg = st.lookup_or_global("x", 99);
        assert_eq!(reg, 3);
    }

    #[test]
    fn var_hoists_to_function_scope() {
        let mut st = SymbolTable::new();
        // Global scope is FunctionScope
        st.declare("x", 0, l(), false).unwrap();
        st.init_var("x");
        // Push a block scope
        st.push_scope();
        // var declaration inside block should go to function scope
        st.declare("y", 1, v(), false).unwrap();
        st.init_var("y");
        st.pop_scope();
        // y should be accessible (it was declared in function scope, not block scope)
        assert_eq!(st.lookup("y").unwrap(), 1);
    }

    #[test]
    fn let_block_scoped() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, l(), false).unwrap();
        st.init_var("x");
        st.push_scope();
        st.declare("x", 1, l(), false).unwrap();
        st.init_var("x");
        assert_eq!(st.lookup("x").unwrap(), 1);
        st.pop_scope();
        assert_eq!(st.lookup("x").unwrap(), 0);
    }

    #[test]
    fn const_has_is_const_flag() {
        let mut st = SymbolTable::new();
        st.declare("x", 0, c(), false).unwrap();
        st.init_var("x");
        let reg = st.lookup("x").unwrap();
        assert_eq!(reg, 0);
    }

    #[test]
    fn var_in_two_blocks_same_function_scope_is_duplicate() {
        let mut st = SymbolTable::new();
        st.push_scope();
        st.declare("y", 1, v(), false).unwrap();
        st.pop_scope();
        st.push_scope();
        // Same function scope, so this should be a duplicate
        assert!(st.declare("y", 2, v(), false).is_err());
        st.pop_scope();
    }
}
