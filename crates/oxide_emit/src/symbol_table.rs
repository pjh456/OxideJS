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
    /// 词法（let/const/class/导入）绑定：写臂谓词据此拒绝全局内置槽解析——
    /// 顶层词法绑定遮蔽同名全局属性，写不落全局对象（var/隐式全局/镜像占位
    /// 不置位，其值存储本就是全局对象属性）。
    pub(crate) lexical: bool,
    /// 由块/函数级预声明（TDZ 占位）创建，声明点据此复用槽位。
    /// 非预声明的同名绑定（如同 scope 的参数/var）不计，避免误复用。
    pub(crate) predeclared: bool,
}

/// 作用域符号表：名字 → 寄存器号/初始化状态/const 标志。
/// 首层为全局函数作用域，后续 push 的为块作用域。
///
/// 别名（`aliases`）承载模块自导入：自导入的局部名是源导出的活引用，其绑定
/// 槽位与源绑定相同，TDZ/提升/活值/不可变四语义全部委托源绑定；表内只负责在
/// 源绑定初始化时同步解除别名 TDZ。
pub struct SymbolTable {
    pub(crate) scopes: Vec<Scope>,
    /// 别名本地名 → 基源绑定名（沿 re-export 链解析到最终源名）。
    aliases: HashMap<String, String>,
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
            aliases: HashMap::new(),
        }
    }

    /// 压入新的块作用域（`let`/`const` 限定于此）。
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            kind: ScopeKind::BlockScope,
        });
    }

    /// 按给定类型压入新作用域（函数或块）。
    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope { bindings: HashMap::new(), kind });
    }

    /// 弹出最内层作用域；全局作用域不可弹出。
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// 从内向外查找最近的函数作用域下标，作为 `var` 的提升目标；全局作用域即函数作用域，故恒能命中，兜底返回 0。
    pub(crate) fn find_var_target_scope(&self) -> usize {
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
                lexical: !matches!(kind, VariableDeclarationKind::Var),
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

    /// 从内向外查绑定并仅返回寄存器号；不检查初始化态，供无需 TDZ 判定的写路径使用。
    pub(crate) fn lookup_any(&self, name: &str) -> Option<u32> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name).map(|binding| binding.reg))
    }

    /// 从内向外查绑定，返回绑定引用及其作用域下标（0 = 全局作用域，供顶层判定）。
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
                lexical: false,
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
                lexical: !matches!(kind, VariableDeclarationKind::Var),
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
            lexical: false,
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
                lexical: !matches!(kind, VariableDeclarationKind::Var),
                predeclared: true,
            },
        );
        Ok(())
    }

    /// 从内到外将同名绑定标记为已初始化（var 提升后初始化阶段使用）。
    /// 指向该源的别名绑定同步置为已初始化，使别名读取的 TDZ 状态跟随源声明点。
    pub fn init_var(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(b) = scope.bindings.get_mut(name) {
                b.initialized = true;
                break;
            }
        }
        // 别名侧表与绑定分居两处，直接按基源名命中全部别名槽位，不走作用域查找。
        let alias_bases: Vec<String> = self
            .aliases
            .iter()
            .filter(|(_, base)| base.as_str() == name)
            .map(|(local, _)| local.clone())
            .collect();
        for local in alias_bases {
            self.mark_initialized(&local);
        }
    }

    /// 声明本地名为 `source` 源绑定的别名：别名槽位复用源寄存器，`is_const`
    /// 恒为真（import 绑定是不可变的间接引用），初始化状态镜像源绑定当前值。
    ///
    /// # 边界与前提
    /// - `source` 经 `aliases` 表解析到基源名（支持别名链）；基源不存在于符号表
    ///   时返回 `Err`，调用方回退非别名路径。
    /// - 别名名在当前最内层作用域已有绑定时返回 `Err`（重复声明）。
    ///
    /// # 副作用
    /// - 在当前最内层作用域插入别名 Binding，并登记 `aliases` 侧表。
    pub(crate) fn add_alias(&mut self, local: &str, source: &str) -> Result<(), String> {
        let base = self.resolve_alias_base(source).to_string();
        let Some((reg, initialized)) = self
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(&base).map(|binding| (binding.reg, binding.initialized)))
        else {
            return Err(format!("alias source '{base}' is not declared"));
        };

        let target = self.scopes.last_mut().expect("symbol table always has a scope");
        if target.bindings.contains_key(local) {
            return Err(format!("Identifier '{local}' has already been declared"));
        }
        target.bindings.insert(
            local.to_string(),
            Binding {
                reg,
                initialized,
                is_const: true,
                // 导入绑定是模块作用域词法（const）绑定，同 let/const 置位。
                lexical: true,
                predeclared: false,
            },
        );
        self.aliases.insert(local.to_string(), base);
        Ok(())
    }

    /// 沿别名侧表把名字解析到最终基源绑定名；非别名名原样返回。
    pub(crate) fn resolve_alias_base<'a>(&'a self, name: &'a str) -> &'a str {
        let mut base = name;
        while let Some(next) = self.aliases.get(base) {
            base = next.as_str();
        }
        base
    }

    /// 把名对应绑定置为已初始化（只查绑定、不问状态）。
    fn mark_initialized(&mut self, name: &str) {
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

    #[test]
    fn alias_shares_source_slot_and_const() {
        let mut st = SymbolTable::new();
        st.declare("x", 3, l(), false).unwrap();
        st.add_alias("y", "x").unwrap();
        // 别名复用源寄存器，且是不可变的间接绑定。
        assert_eq!(st.lookup_any("y"), Some(3));
        assert!(st.lookup_any_binding("y").unwrap().0.is_const);
        // 源未初始化时别名读同样报 TDZ。
        assert!(st.lookup("y").unwrap_err().contains("before initialization"));
    }

    #[test]
    fn alias_tdz_follows_source_init() {
        let mut st = SymbolTable::new();
        st.declare("x", 3, l(), false).unwrap();
        st.add_alias("y", "x").unwrap();
        st.init_var("x");
        // 源声明点初始化同步解除别名 TDZ。
        assert_eq!(st.lookup("y").unwrap(), 3);
    }

    #[test]
    fn alias_chain_resolves_to_base() {
        let mut st = SymbolTable::new();
        st.declare("x", 3, l(), false).unwrap();
        st.add_alias("y", "x").unwrap();
        st.add_alias("z", "y").unwrap();
        assert_eq!(st.resolve_alias_base("z"), "x");
        assert_eq!(st.lookup_any("z"), Some(3));
    }

    #[test]
    fn alias_of_missing_source_is_error() {
        let mut st = SymbolTable::new();
        assert!(st.add_alias("y", "missing").is_err());
    }
}
