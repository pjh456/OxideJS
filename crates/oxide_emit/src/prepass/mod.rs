//! emit 前置 pass：builtin 引用预扫描 + 声明预登记。
//!
//! 在生成临时寄存器前遍历 AST，把内置全局标识符预先登记到固定寄存器槽
//! （builtin_reg_map），避免与临时寄存器池冲突；同时预声明函数/var 声明，
//! 支持提升语义。

mod block_fn;
mod builtin_scan;
mod declare;

/// 脚本顶层 lexical 声明（let/const/class）禁止使用的受限全局名：3 名静态集。
/// 受限判定走规范自有属性臂：全局对象上 {configurable:false} 自有属性名（声明
/// 实例化无法建立遮蔽绑定）——现行规范下恰为三常量；eval 与各 builtin 属性皆
/// 可配置（合法遮蔽），不在此列。名基静态集（编译期不可查运行时描述符）。与
/// BUILTIN_GLOBALS 语义不同（后者是 put 写拦截/双写名单），不互相派生，交叠
/// 名由漂移守卫单测断言恒同步。
pub(crate) const RESTRICTED_GLOBAL_LEXICAL_NAMES: &[&str] = &["undefined", "NaN", "Infinity"];

/// 脚本顶层 lexical 声明撞受限全局名 → SyntaxError（声明实例化期拒绝，整程序
/// 编译失败）。错误消息与既有重复声明错同形；非顶层或名不在受限集 → Ok。
fn check_restricted_global_lexical(name: &str, global_lexical: bool) -> Result<(), String> {
    if global_lexical && RESTRICTED_GLOBAL_LEXICAL_NAMES.contains(&name) {
        return Err(format!("Identifier '{name}' has already been declared"));
    }
    Ok(())
}
