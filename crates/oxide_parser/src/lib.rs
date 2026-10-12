#![doc = "OxideJS - JavaScript parser (oxc_parser re-export)"]

mod error;
mod parser_log;
mod redeclaration_filter;

use std::cell::{Cell, UnsafeCell};

/// 解析错误：消息 + 源文件字节区间（起始，结束）。
pub use error::OxideError;
/// 重导出 `oxc_allocator::Allocator`：AST 节点的 bump 分配器，解析产物生命周期由其决定。
pub use oxc_allocator::Allocator;
/// 重导出 `oxc_ast::ast` 的全部 AST 节点类型（`Program`、`Statement`、`Expression` 等）。
pub use oxc_ast::ast::*;
/// 重导出 `oxc_span::Span`：源码中的位置区间。
pub use oxc_span::Span;

// 线程本地持久 bump 分配器：解析入口共享该分配器，chunk 内存按线程
// 高水位保留复用，稳态解析不再走全局分配器。
//
// 分配器存于 `UnsafeCell`：唯一可变操作 `reset()` 只在最外层（深度为零）
// 调用，彼时尚无共享引用存活；闭包执行期间只产生共享 `&Allocator` 引用
// （重入不再 reset），共享引用可别名，无 `&`/`&mut` 混用。
// 重入深度独立存于 `Cell`（`Copy` 类型内部可变），嵌套读写不产生借用冲突。
thread_local! {
    static PARSE_ALLOCATOR: UnsafeCell<Allocator> = UnsafeCell::new(Allocator::new());
    static PARSE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// 重入深度守卫：出口（含 panic 展开）把深度减一，防深度泄漏。
struct DepthGuard<'a>(&'a Cell<usize>);

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// 在线程本地分配器上执行一次解析上下文：闭包内完成解析与编译，
/// AST 生命周期被闭包边界约束。
///
/// # 边界与前提
/// - 闭包不得把 AST 逃逸出闭包边界：下一次顶层入口会 reset 分配器。
/// - 闭包内重入本助手（嵌套解析）安全：深度非零时跳过 reset，
///   嵌套解析共享同一分配器，AST 在外层退出前全部有效。
///
/// # 副作用
/// - 深度为零时 reset 分配器（bump 指针归零，高水位 chunk 保留）；
///   重入深度入口加一、出口减一（含 panic 展开路径）。
pub fn with_parse_allocator<R, F>(f: F) -> R
where
    F: for<'a> FnOnce(&'a Allocator) -> R,
{
    PARSE_DEPTH.with(|depth| {
        let is_root = depth.get() == 0;
        depth.set(depth.get() + 1);
        let _guard = DepthGuard(depth);
        PARSE_ALLOCATOR.with(|cell| {
            let ptr = cell.get();
            // reset 只在最外层执行：此刻尚无共享引用存活，`&mut` 独占安全。
            if is_root {
                unsafe { (*ptr).reset() };
            }
            // 共享引用跨闭包存活；重入只再产生共享引用，无 `&mut` 混用。
            let alloc = unsafe { &*ptr };
            f(alloc)
        })
    })
}

/// 把 `source` 解析为 AST 根节点 `Program`，AST 分配在 `allocator` 中。
///
/// 使用 oxc 默认 `SourceType`（严格模式由语法自身决定）。返回 `Ok(program)`
/// 或 `Err`（收集全部解析错误，按源码顺序排列）；若 oxc 内部 panic（不可恢复
/// 语法错误）则透传其已收集的诊断（消息 + 位置），无诊断时返回单个通用错误。
fn parse_with_source_type<'a>(
    allocator: &'a Allocator, source: &'a str, source_type: oxc_span::SourceType,
) -> Result<Program<'a>, Vec<OxideError>> {
    use oxc_parser::Parser;
    use oxc_semantic::Stats;

    let ret = Parser::new(allocator, source, source_type).parse();

    if ret.panicked {
        // 不可恢复语法错误：oxc 已为出错 token 累积诊断时，把诊断（消息 + 位置）
        // 透传为 `Err`，直指真实触发点；无诊断时兜底通用文本。
        if !ret.errors.is_empty() {
            return Err(ret.errors.into_iter().map(OxideError::from).collect());
        }
        return Err(vec![OxideError {
            message: "Parser panicked: unrecoverable syntax error".to_string(),
            span: (0, 0),
        }]);
    }

    if !ret.errors.is_empty() {
        return Err(ret.errors.into_iter().map(OxideError::from).collect());
    }

    // 语法早期错误（Early Errors）检查：oxc parser 把复杂早期错误委托给
    // oxc_semantic（strict 绑定标识符、重复参数、NSPL、super 位置等）。

    // 节点、作用域、符号、引用四计数各占至少一个源字节，源长加一即为四者的
    // 安全上界；传入后语义构建器跳过 Stats::count 全 AST 遍历，直接按上界
    // 预分配容量。
    let bound = (source.len() + 1).min(u32::MAX as usize) as u32;
    let semantic_ret = oxc_semantic::SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_stats(Stats::new(bound, bound, bound, bound))
        .build(&ret.program);
    if !semantic_ret.errors.is_empty() {
        // 重名诊断先经 AST 复核剔除跨语句列合法合并的误报（须在归一化前，
        // 归一化只留首标签会丢失复核所需的新声明 span）；复核后无剩余
        // 错误时按解析成功落回。
        let filtered = redeclaration_filter::filter_semantic_diagnostics(semantic_ret.errors, &ret.program);
        if !filtered.is_empty() {
            return Err(filtered.into_iter().map(OxideError::from).collect());
        }
    }

    Ok(ret.program)
}

/// 将 `source` 解析为 AST 根节点 `Program`（脚本/模块由语法内容自动判定）。
///
/// 使用 oxc 的 `unambiguous` SourceType：代码含顶层 import/export 时按模块解析，
/// 否则按脚本解析。返回 `Ok(program)` 或 `Err`（收集全部解析错误）。
pub fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    parse_with_source_type(allocator, source, oxc_span::SourceType::unambiguous())
}

/// 将 `source` 强制按 ES module 解析为 AST 根节点 `Program`。
///
/// 与 [`parse`] 的唯一区别是 SourceType 固定为模块（`SourceType::mjs`）：
/// 即使源码没有 import/export 也会按模块文法解析（如 `await` 保留字、
/// module-only early errors 等），用于 test262 `flags: [module]` 用例。
pub fn parse_module<'a>(allocator: &'a Allocator, source: &'a str) -> Result<Program<'a>, Vec<OxideError>> {
    parse_with_source_type(allocator, source, oxc_span::SourceType::mjs())
}
