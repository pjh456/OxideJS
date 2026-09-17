//! emit 前置 pass：builtin 引用预扫描 + 声明预登记。
//!
//! 在生成临时寄存器前遍历 AST，把内置全局标识符预先登记到固定寄存器槽
//! （builtin_reg_map），避免与临时寄存器池冲突；同时预声明函数/var 声明，
//! 支持提升语义。

mod block_fn;
mod builtin_scan;
mod declare;

/// 脚本顶层词法声明（`let`/`const`/`class`）禁止使用的受限全局名：`undefined`、
/// `NaN`、`Infinity` 三名静态集。
///
/// 判定标准是全局对象上 `configurable:false` 的自有属性名：此类属性无法由声明
/// 实例化建立遮蔽绑定，现行规范下恰为三常量；`eval` 与其余内置属性皆可配置，
/// 可合法遮蔽。运行时无法查询全局对象描述符，名单必须编译期确定。
///
/// 与 `BUILTIN_GLOBALS` 语义不同：后者是写入拦截与双写名单，两份名单互不派生，
/// 交叠名由 `restricted_lexical_names_within_builtin_globals` 单测断言恒同步。
pub(crate) const RESTRICTED_GLOBAL_LEXICAL_NAMES: &[&str] = &["undefined", "NaN", "Infinity"];

/// 脚本顶层 lexical 声明撞受限全局名 → SyntaxError（声明实例化期拒绝，整程序
/// 编译失败）。错误消息与既有重复声明错同形；非顶层或名不在受限集 → Ok。
fn check_restricted_global_lexical(name: &str, global_lexical: bool) -> Result<(), String> {
    if global_lexical && RESTRICTED_GLOBAL_LEXICAL_NAMES.contains(&name) {
        return Err(format!("Identifier '{name}' has already been declared"));
    }
    Ok(())
}
