use std::collections::HashMap;

pub(crate) struct Binding {
    pub(crate) reg: u8,
    pub(crate) initialized: bool,
}

pub struct SymbolTable {
    pub(crate) scopes: Vec<HashMap<String, Binding>>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn declare(&mut self, name: &str, reg: u8) -> Result<(), String> {
        let current = self.scopes.last_mut().unwrap();
        if current.contains_key(name) {
            return Err(format!("Identifier '{name}' has already been declared"));
        }
        current.insert(
            name.to_string(),
            Binding {
                reg,
                initialized: false,
            },
        );
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Result<u8, String> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
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
            if let Some(b) = scope.get(name) {
                return b.reg;
            }
        }
        self.scopes[0].insert(
            name.to_string(),
            Binding {
                reg: reg_for_new,
                initialized: true,
            },
        );
        reg_for_new
    }

    /// Declare a variable that is already initialized (for FunctionDeclaration hoisting).
    /// If the name already exists in the current scope, just mark it as initialized.
    pub fn declare_initialized(&mut self, name: &str, reg: u8) -> Result<(), String> {
        let current = self.scopes.last_mut().unwrap();
        if let Some(b) = current.get_mut(name) {
            b.initialized = true;
            return Ok(());
        }
        current.insert(
            name.to_string(),
            Binding {
                reg,
                initialized: true,
            },
        );
        Ok(())
    }

    pub fn pre_register_global(&mut self, name: &str, reg: u8) {
        self.scopes[0].entry(name.to_string()).or_insert(Binding {
            reg,
            initialized: true,
        });
    }

    pub fn init_var(&mut self, name: &str) {
        let current = self.scopes.last_mut().unwrap();
        if let Some(b) = current.get_mut(name) {
            b.initialized = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolTable;

    #[test]
    fn global_scope_exists() {
        let st = SymbolTable::new();
        assert!(st.lookup("x").is_err());
    }

    #[test]
    fn declare_and_lookup() {
        let mut st = SymbolTable::new();
        st.declare("x", 0).unwrap();
        st.init_var("x");
        assert_eq!(st.lookup("x").unwrap(), 0);
    }

    #[test]
    fn tdz_error() {
        let mut st = SymbolTable::new();
        st.declare("x", 0).unwrap();
        assert!(st
            .lookup("x")
            .unwrap_err()
            .contains("before initialization"));
    }

    #[test]
    fn duplicate_declaration_error() {
        let mut st = SymbolTable::new();
        st.declare("x", 0).unwrap();
        assert!(st.declare("x", 1).is_err());
    }

    #[test]
    fn nested_scopes_shadow() {
        let mut st = SymbolTable::new();
        st.declare("x", 0).unwrap();
        st.init_var("x");
        st.push_scope();
        st.declare("x", 1).unwrap();
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
        st.declare("x", 3).unwrap();
        st.init_var("x");
        let reg = st.lookup_or_global("x", 99);
        assert_eq!(reg, 3);
    }
}
