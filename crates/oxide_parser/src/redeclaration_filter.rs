//! 语义早期错误诊断的 AST 复核：剔除函数名与 var 声明合法合并的重名误报。
//!
//! "Identifier `x` has already been declared" 诊断按"声明子当前作用域 +
//! 合并符号历史"粒度发射：var 声明子位于语句作用域（for-in/for-of 头、块、
//! switch 等）且其 var 作用域内同名合并项的上一项为函数声明即判碰撞。
//! 规范按"语句列"粒度裁定：只有作为**某个块/case 语句列直接成员**的函数
//! 声明才计入该列的 lexically declared 名，其与该列闭包内 var 声明同名
//! 才是 SyntaxError；脚本顶层/函数体语句列直接成员的函数声明与同 var
//! 作用域的 var 声明属提升后合法合并（var 步复用既有绑定），不应拒。
//!
//! 关键不变式：只剔除可确认误报的既定消息形诊断；消息形不匹配、标签
//! 缺失、模块模式、节点匹配失败一律原样保留，不吞任何真错。

use std::collections::HashMap;

use oxc_ast::ast::{
    BlockStatement, Function, FunctionType, IfStatement, Program, SwitchStatement, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::walk::{
    walk_block_statement, walk_function_body, walk_if_statement, walk_program, walk_switch_statement,
    walk_variable_declaration,
};
use oxc_ast_visit::Visit;
use oxc_diagnostics::OxcDiagnostic;
use oxc_syntax::scope::ScopeFlags;

/// 重名诊断消息形：前缀 + 标识符名 + 后缀。
///
/// 上游改文案时匹配失败、过滤静默透传（行为回到全量拒绝，不误吞真错），
/// 消息形由测试钉锁。
const MSG_PREFIX: &str = "Identifier `";
const MSG_SUFFIX: &str = "` has already been declared";

/// 对语义诊断列表做重名误报复核，返回剔除误报后的列表。
///
/// 须在 `OxideError` 归一化之前调用：归一化只保留首个标签，会丢失
/// "新声明"侧 span 而失去复核能力。
///
/// # 边界与前提
/// - 诊断列表可能混有非重名形诊断：逐条独立裁定，未命中剔除条件者原样保留。
/// - 模块模式（`program.source_type` 为模块）顶层函数名为 lexical 绑定，
///   一切 var 碰撞均为真错，整列保留。
///
/// # 注意事项
/// - 无重名形诊断时直接返回原列表（常见路径零开销）。
/// - AST 遍历只读，不改写 `program`。
pub(crate) fn filter_semantic_diagnostics<'a>(errors: Vec<OxcDiagnostic>, program: &Program<'a>) -> Vec<OxcDiagnostic> {
    if !errors.iter().any(|e| redeclaration_name(&e.message).is_some()) {
        return errors;
    }

    // 单遍收集函数声明归属与 var 声明块链，供逐诊断复核。
    let mut collector = Collector::default();
    collector.visit_program(program);

    let is_module = program.source_type.is_module();
    errors
        .into_iter()
        .filter(|e| verdict(e, &collector, is_module) != Some(true))
        .collect()
}

/// 从重名形消息提取标识符名；消息形不匹配返 `None`。
fn redeclaration_name(message: &str) -> Option<&str> {
    let name = message.strip_prefix(MSG_PREFIX)?;
    name.strip_suffix(MSG_SUFFIX)
}

/// 裁定单条重名形诊断。
///
/// 返回 `Some(true)` = 已确认误报（可剔除）；`Some(false)` = 已确认真错
/// （保留）；`None` = 不在本过滤裁定范围（消息形/标签/节点匹配失败），
/// 一律原样保留。
///
/// # 步骤
/// 1. 消息形匹配取标识符名；标签取双标签（0 = 既有声明，1 = 新声明）。
/// 2. 既有声明侧按 span 匹配同名函数声明，取其语句列归属。
/// 3. 新声明侧在标签区间内匹配同名 var 绑定，取其块/case 链。
/// 4. 函数不在任何块/case 语句列直接成员位 → 误报（合法合并形）；
///    在 → 真错当且仅当 var 声明位于该语句列闭包内（块链含该列）。
fn verdict(diag: &OxcDiagnostic, col: &Collector<'_>, is_module: bool) -> Option<bool> {
    let name = redeclaration_name(&diag.message)?;
    let labels = diag.labels.as_deref()?;
    let (existing, new) = match (labels.first(), labels.get(1)) {
        (Some(a), Some(b)) => (a, b),
        _ => return None,
    };
    // 模块顶层函数名为 lexical 绑定，一切 var 碰撞均为真错。
    if is_module {
        return Some(false);
    }

    let (fn_name, owner) = col.fn_owners.get(&label_key(existing))?;
    if *fn_name != name {
        return None;
    }

    // 新声明标签可覆盖整个声明子（含初始化器），绑定名 span 必为其子区间。
    let (start, end) = (new.offset() as u32, new.offset() as u32 + new.len() as u32);
    let chain = col
        .var_chains
        .iter()
        .find(|(span, ident, _)| *ident == name && span.0 >= start && span.1 <= end)?
        .2
        .clone();

    Some(match owner {
        None => true,
        Some(list) => !chain.contains(list),
    })
}

/// span 编码为单键：起点高 32 位、终点低 32 位。
fn label_key(label: &oxc_diagnostics::LabeledSpan) -> u64 {
    let start = label.offset() as u64;
    let end = start + label.len() as u64;
    (start << 32) | end
}

/// AST 收集器：记录复核所需的声明归属信息。
///
/// 归属性状语义（随遍历位置变化）：
/// - `block_chain`：当前位置外部的块/case 容器列（最外层在前）；
/// - `direct_owner`：当前语句所在语句列的块/switch 归属（程序体、函数体、
///   static block、if/else 臂、标签语句等非块语句列为 `None`）。
#[derive(Default)]
struct Collector<'a> {
    /// 函数声明名 span → (名, 函数声明直接归属的语句列)；后者 `None` 表示
    /// 直接位于程序体/函数体/static block 或非块语句（if/else 臂、标签语句），
    /// 这些位置的函数名不计入任何语句列的 lexically declared 名。
    fn_owners: HashMap<u64, (&'a str, Option<u64>)>,
    /// var 声明绑定名：(span, 名, 该声明处的块/case 链)；覆盖解构的全部绑定。
    var_chains: Vec<((u32, u32), &'a str, Vec<u64>)>,
    /// 当前位置外部的块/switch 容器 span 列（最外层在前）。
    block_chain: Vec<u64>,
    /// 当前语句直接所在语句列的块/switch span；非块语句列为 `None`。
    direct_owner: Option<u64>,
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_program(&mut self, program: &Program<'a>) {
        // 程序语句列是脚本 var 作用域的顶层语句列：两项归属皆空。
        let saved = (std::mem::take(&mut self.block_chain), self.direct_owner);
        self.direct_owner = None;
        walk_program(self, program);
        self.block_chain = saved.0;
        self.direct_owner = saved.1;
    }

    fn visit_function(&mut self, func: &Function<'a>, _flags: ScopeFlags) {
        // 函数声明按当前 direct_owner 归属记录（仅块/switch 语句列有归属）。
        if func.r#type == FunctionType::FunctionDeclaration {
            if let Some(id) = &func.id {
                let span = id.span;
                self.fn_owners
                    .insert(((span.start as u64) << 32) | span.end as u64, (id.name.as_str(), self.direct_owner));
            }
        }

        // 参数默认值为表达式上下文：不含语句列归属变化。
        self.visit_formal_parameters(&func.params);

        // 函数体语句列是新 var 作用域的顶层语句列：两项归属复位。
        if let Some(body) = &func.body {
            let saved = (std::mem::take(&mut self.block_chain), self.direct_owner);
            self.direct_owner = None;
            walk_function_body(self, body);
            self.block_chain = saved.0;
            self.direct_owner = saved.1;
        }
    }

    fn visit_block_statement(&mut self, block: &BlockStatement<'a>) {
        // 块是重名裁定列：其直接成员的函数声明计入该列 lexically declared 名，
        // 闭包内（含嵌套块）的 var 声明计入该列 VarDeclaredNames。
        let key = (block.span.start as u64) << 32 | block.span.end as u64;
        let saved_len = self.block_chain.len();
        let saved_owner = self.direct_owner;
        self.block_chain.push(key);
        self.direct_owner = Some(key);
        walk_block_statement(self, block);
        self.block_chain.truncate(saved_len);
        self.direct_owner = saved_owner;
    }

    fn visit_if_statement(&mut self, if_stmt: &IfStatement<'a>) {
        // if/else 臂非重名裁定语句列：臂内函数声明按外层 var 作用域绑定，
        // 其名不计入任何语句列的 lexically declared 名（标签语句不同：
        // 其内层语句仍为原语句列直接成员，归属继承不动）。
        let saved = self.direct_owner;
        self.direct_owner = None;
        walk_if_statement(self, if_stmt);
        self.direct_owner = saved;
    }

    fn visit_switch_statement(&mut self, switch: &SwitchStatement<'a>) {
        // switch 的 CaseBlock 整体是重名裁定列：任一 case 的函数声明与
        // 任一 case 的 var 声明同列，等同同块碰撞。
        let key = (switch.span.start as u64) << 32 | switch.span.end as u64;
        let saved_len = self.block_chain.len();
        let saved_owner = self.direct_owner;
        self.block_chain.push(key);
        self.direct_owner = Some(key);
        walk_switch_statement(self, switch);
        self.block_chain.truncate(saved_len);
        self.direct_owner = saved_owner;
    }

    fn visit_variable_declaration(&mut self, decl: &VariableDeclaration<'a>) {
        // 仅 var 声明是重名诊断的"新声明"侧（let/const/using 各有其裁定臂）。
        if decl.kind == VariableDeclarationKind::Var {
            for d in &decl.declarations {
                let chain = self.block_chain.clone();
                for ident in d.id.get_binding_identifiers() {
                    self.var_chains
                        .push(((ident.span.start, ident.span.end), ident.name.as_str(), chain.clone()));
                }
            }
        }
        walk_variable_declaration(self, decl);
    }
}
