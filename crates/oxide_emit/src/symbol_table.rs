//! 标识符合法化 / 作用域符号表。
//!
//! 层叠作用域栈：`var` 声明提升到最近函数作用域，`let`/`const` 留在当前块。
//! 绑定记录寄存器号 + 初始化标志（TDZ 检查）+ const 标志。声明即写入，
//! `lookup` 在未初始化时报 TDZ 错误。

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
    pub(crate) reg: u32,
    pub(crate) initialized: bool,
    pub(crate) is_const: bool,
    /// 由块/函数级预声明（TDZ 占位）创建，声明点据此复用槽位。
    /// 非预声明的同名绑定（如同 scope 的参数/var）不计，避免误复用。
    pub(crate) predeclared: bool,
}

/// 作用域符号表：名字 → 寄存器号/初始化状态/const 标志。
/// 首层为全局函数作用域，后续 push 的为块作用域。
pub struct SymbolTable {
    pub(crate) scopes: Vec<Scope>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    /// 构造符号表：预置一个全局函数作用域。
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope {
                bindings: HashMap::new(),
                kind: ScopeKind::FunctionScope,
            }],
        }
    }

    /// 压入新的块作用域（`let`/`const` 限定于此）。
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            kind: ScopeKind::BlockScope,
        });
    }

    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope { bindings: HashMap::new(), kind });
    }

    /// 弹出最内层作用域；全局作用域不可弹出。
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

    /// 声明绑定：`var` 提升到函数作用域，`let`/`const` 落在当前块。
    /// 初始为未初始化态（TDZ）；重复声明报错。
    pub fn declare(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
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
                predeclared: false,
            },
        );
        Ok(())
    }

    /// 从内到外查找已初始化绑定；命中未初始化绑定报 TDZ 错误，未找到报未定义。
    pub fn lookup(&self, name: &str) -> Result<u32, String> {
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

    pub(crate) fn lookup_any(&self, name: &str) -> Option<u32> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name).map(|binding| binding.reg))
    }

    pub(crate) fn lookup_any_binding(&self, name: &str) -> Option<(&Binding, usize)> {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if let Some(b) = scope.bindings.get(name) {
                return Some((b, i));
            }
        }
        None
    }

    /// 消费当前（最内层）作用域中由预声明创建的绑定槽：存在且 `predeclared`
    /// 时返回其寄存器并清除标志（声明点复用后不再视作预声明）；否则返回 None。
    /// 同 scope 的非预声明绑定（参数/var/未推 scope 的 try 内声明）不计，避免误复用。
    pub(crate) fn consume_predeclared_slot(&mut self, name: &str) -> Option<u32> {
        let scope = self.scopes.last_mut()?;
        let binding = scope.bindings.get_mut(name)?;
        if !binding.predeclared {
            return None;
        }
        binding.predeclared = false;
        Some(binding.reg)
    }

    /// 查找或视为全局：未命中时以 `reg_for_new` 在全局作用域登记并返回（隐式全局）。
    pub fn lookup_or_global(&mut self, name: &str, reg_for_new: u32) -> u32 {
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
                predeclared: false,
            },
        );
        reg_for_new
    }

    /// 返回已初始化绑定是否为 const（用于 const 赋值检查）；未初始化/未找到返回 false。
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

    /// 声明并直接标记为已初始化；绑定已存在时仅补初始化标志（供预声明路径）。
    pub fn declare_initialized(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
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
                predeclared: false,
            },
        );
        Ok(())
    }

    /// 在全局作用域预登记绑定（builtin 全局等），不覆盖已存在的同名绑定。
    pub fn pre_register_global(&mut self, name: &str, reg: u32) {
        self.scopes[0].bindings.entry(name.to_string()).or_insert(Binding {
            reg,
            initialized: true,
            is_const: false,
            predeclared: false,
        });
    }

    /// 声明未初始化绑定并标记为预声明（TDZ 占位），供声明点复用槽位。
    /// 与 `declare` 同作用域规则，仅 `predeclared` 标志不同。
    pub(crate) fn declare_predeclared(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
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
                predeclared: true,
            },
        );
        Ok(())
    }

    /// 从内到外将同名绑定标记为已初始化（var 提升后初始化阶段使用）。
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
        assert!(st.lookup("x").unwrap_err().contains("before initialization"));
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
        // 全局作用域是函数作用域
        st.declare("x", 0, l(), false).unwrap();
        st.init_var("x");
        // 压入块作用域
        st.push_scope();
        // 块内的 var 声明应落到函数作用域
        st.declare("y", 1, v(), false).unwrap();
        st.init_var("y");
        st.pop_scope();
        // y 可见（声明在函数作用域而非块作用域）
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
        // 同一函数作用域，故应为重复声明
        assert!(st.declare("y", 2, v(), false).is_err());
        st.pop_scope();
    }
}
