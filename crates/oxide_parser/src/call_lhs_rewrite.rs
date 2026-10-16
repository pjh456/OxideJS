//! 语句起始位裸 CallExpression 赋值目标的源文本等价改写。
//!
//! 非严格（sloppy）模式下，`f() = g()`、`f() += g()`、`f()++`、`++f()`、
//! `for (f() in/of …)` 这类以裸 CallExpression 作赋值目标的写法在规范里是
//! web-compat：求值左侧调用后抛 `ReferenceError`，而非解析期 `SyntaxError`。
//! oxc 解析器把该形式按 `SyntaxError` 致命拒绝（整个 AST 丢弃），本模块在
//! 解析前对源文本做等价改写，把目标形式替换为先执行调用、再抛
//! `ReferenceError` 的语句序列，使求值序与断言面保持逐一对应。
//!
//! 改写只作用于脚本入口（模块恒严格，不触发）；严格脚本（首个语句为
//! `"use strict"` 指令）与严格函数体（函数体首语句为 `"use strict"` 指令）
//! 一律不改写，维持解析拒绝。任何未确认可改写的情形一律不改写（回退解析
//! 拒绝，安全方向：仅漏改，不致回归）。

use std::borrow::Cow;

/// 改写入口：返回改写后的源文本；无改写时返回原引用（零拷贝快速路径）。
pub fn rewrite_call_lhs(source: &str) -> Cow<'_, str> {
    // 词法扫描失败（未闭合字符串/模板/注释等）时整体不改写，回退解析拒绝。
    let toks = match tokenize(source) {
        Some(t) => t,
        None => return Cow::Borrowed(source),
    };

    // ES 模块（顶层 import/export 声明）恒严格，解析拒绝即规范正确，不改写。
    if has_top_level_module_syntax(&toks, source) {
        return Cow::Borrowed(source);
    }

    // 严格脚本（首个语句为 "use strict" 指令）整体不改写。
    if is_directive_strict(&toks, source, 0) {
        return Cow::Borrowed(source);
    }

    // 收集严格函数体区间（函数体首语句为 "use strict" 指令），区间内不改写。
    let strict_regions = collect_strict_regions(&toks, source);

    // 逐语句扫描，收集 (起始字节, 结束字节, 替换文本) 改写区间。
    let rewrites = scan_rewrites(&toks, source, &strict_regions);
    if rewrites.is_empty() {
        return Cow::Borrowed(source);
    }

    // 区间按升序、互不重叠，从后往前拼接出新文本。
    let mut out = String::with_capacity(source.len());
    let mut last = 0usize;
    for (start, end, repl) in &rewrites {
        out.push_str(&source[last..*start]);
        out.push_str(repl);
        last = *end;
    }
    out.push_str(&source[last..]);
    Cow::Owned(out)
}

// ─────────────────────────── 词法 ───────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ident,
    Str,
    Template,
    Num,
    Punct,
}

struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
}

fn tok_val<'a>(src: &'a str, t: &Tok) -> &'a str {
    &src[t.start..t.end]
}

/// 把源文本切成 token 序列；字符串/模板/注释未闭合时返回 `None`（调用方回退
/// 不改写）。注释不产生 token，直接跳过。
fn tokenize(src: &str) -> Option<Vec<Tok>> {
    let b = src.as_bytes();
    let n = b.len();
    let mut toks = Vec::new();
    let mut i = 0usize;

    while i < n {
        let c = b[i];

        // 空白
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // 行注释
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // 块注释
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 >= n {
                return None;
            }
            i += 2;
            continue;
        }

        // 字符串字面量
        if c == b'"' || c == b'\'' {
            let s = i;
            i = scan_string(b, i)?;
            toks.push(Tok {
                kind: Kind::Str,
                start: s,
                end: i,
            });
            continue;
        }

        // 模板字面量（含 ${} 嵌套表达式）
        if c == b'`' {
            let s = i;
            i = scan_template(b, i)?;
            toks.push(Tok {
                kind: Kind::Template,
                start: s,
                end: i,
            });
            continue;
        }

        // 正则字面量（与除法的歧义落「不重写」侧：按除法处理）
        if c == b'/' && can_start_regex(&toks, src) {
            let s = i;
            i = scan_regex(b, i)?;
            toks.push(Tok {
                kind: Kind::Punct,
                start: s,
                end: i,
            });
            continue;
        }

        // 数字字面量
        if c.is_ascii_digit() || (c == b'.' && i + 1 < n && b[i + 1].is_ascii_digit()) {
            let s = i;
            i = scan_number(b, i);
            toks.push(Tok {
                kind: Kind::Num,
                start: s,
                end: i,
            });
            continue;
        }

        // 标识符（含关键字）
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let s = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            toks.push(Tok {
                kind: Kind::Ident,
                start: s,
                end: i,
            });
            continue;
        }

        // 标点/运算符（最长匹配）
        let len = match_punct(b, i);
        if len == 0 {
            return None;
        }
        let s = i;
        i += len;
        toks.push(Tok {
            kind: Kind::Punct,
            start: s,
            end: i,
        });
    }

    Some(toks)
}

/// 从开引号位置扫描字符串，返回闭引号后的下标；未闭合返回 `None`。
fn scan_string(b: &[u8], mut i: usize) -> Option<usize> {
    let q = b[i];
    let n = b.len();
    i += 1;
    loop {
        if i >= n || b[i] == b'\n' {
            return None;
        }
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            return Some(i + 1);
        }
        i += 1;
    }
}

/// 从开反引号扫描模板字面量，返回闭反引号后的下标；未闭合返回 `None`。
/// `${}` 内是普通表达式上下文（含嵌套括号/字符串/模板），用栈逐层跟踪。
fn scan_template(b: &[u8], mut i: usize) -> Option<usize> {
    let n = b.len();
    i += 1;
    // 栈内每层是 ${} 表达式的括号深度；栈空表示当前在模板文本段。
    let mut depths: Vec<usize> = Vec::new();

    loop {
        if i >= n {
            return None;
        }
        let c = b[i];

        if depths.is_empty() {
            match c {
                b'\\' => i += 2,
                b'`' => return Some(i + 1),
                b'$' if i + 1 < n && b[i + 1] == b'{' => {
                    depths.push(0);
                    i += 2;
                }
                _ => i += 1,
            }
            continue;
        }

        match c {
            b'{' => *depths.last_mut().unwrap() += 1,
            b'}' => {
                if *depths.last().unwrap() == 0 {
                    depths.pop();
                } else {
                    *depths.last_mut().unwrap() -= 1;
                }
            }
            b'`' => i = scan_template(b, i)?,
            b'"' | b'\'' => i = scan_string(b, i)?,
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 >= n {
                    return None;
                }
                i += 2;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

/// 从开斜杠扫描正则字面量（含字符类与转义），返回标志位后的下标；未闭合返回
/// `None`。
fn scan_regex(b: &[u8], mut i: usize) -> Option<usize> {
    let n = b.len();
    i += 1;
    let mut in_class = false;
    loop {
        if i >= n {
            return None;
        }
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == b'\n' {
            return None;
        }
        if in_class {
            if c == b']' {
                in_class = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'[' => in_class = true,
            b'/' => {
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    while i < n && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    Some(i)
}

/// 从数字起始位置扫描数字字面量（含进制前缀/指数/bigint 后缀），返回结束下标。
fn scan_number(b: &[u8], mut i: usize) -> usize {
    let n = b.len();
    if b[i] == b'0' && i + 1 < n && (b[i + 1] == b'x' || b[i + 1] == b'o' || b[i + 1] == b'b') {
        i += 2;
        while i < n && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
            i += 1;
        }
    } else {
        while i < n && (b[i].is_ascii_digit() || b[i] == b'_' || b[i] == b'.') {
            i += 1;
        }
        if i < n && (b[i] == b'e' || b[i] == b'E') {
            i += 1;
            if i < n && (b[i] == b'+' || b[i] == b'-') {
                i += 1;
            }
            while i < n && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    if i < n && b[i] == b'n' {
        i += 1;
    }
    i
}

/// 标点/运算符最长匹配：依次试三字符、两字符、单字符，返回匹配长度（0 表示
/// 未知字符）。
fn match_punct(b: &[u8], i: usize) -> usize {
    let n = b.len();
    let three = |s: &str| i + 3 <= n && &b[i..i + 3] == s.as_bytes();
    let two = |s: &str| i + 2 <= n && &b[i..i + 2] == s.as_bytes();
    let one = |s: &str| i < n && b[i] == s.as_bytes()[0];

    if three("===") || three("!==") || three("**=") || three(">>=") || three("...") {
        return 3;
    }
    if two("==")
        || two("!=")
        || two("<=")
        || two(">=")
        || two("&&")
        || two("||")
        || two("??")
        || two("=>")
        || two("+=")
        || two("-=")
        || two("*=")
        || two("/=")
        || two("%=")
        || two("&=")
        || two("|=")
        || two("^=")
        || two("<<")
        || two(">>")
        || two(">>=")
        || two("<<=")
        || two("**")
        || two("++")
        || two("--")
        || two("?.")
        || two("&&=")
        || two("||=")
        || two("??=")
    {
        return 2;
    }
    if one("+")
        || one("-")
        || one("*")
        || one("/")
        || one("%")
        || one("&")
        || one("|")
        || one("^")
        || one("!")
        || one("~")
        || one("?")
        || one(":")
        || one(".")
        || one(",")
        || one(";")
        || one("(")
        || one(")")
        || one("[")
        || one("]")
        || one("{")
        || one("}")
        || one("<")
        || one(">")
        || one("=")
    {
        return 1;
    }
    0
}

/// 正则字面量起始判定：前一显著 token 不是标识符/数字/字符串/模板/右括号/
/// 自增自减时，`/` 按正则处理；否则按除法（歧义落「不重写」侧）。
fn can_start_regex(toks: &[Tok], src: &str) -> bool {
    let Some(p) = toks.last() else {
        return true;
    };
    match p.kind {
        Kind::Ident => {
            // 仅关键字之后可起正则
            matches!(
                tok_val(src, p),
                "return"
                    | "typeof"
                    | "instanceof"
                    | "in"
                    | "of"
                    | "new"
                    | "delete"
                    | "void"
                    | "throw"
                    | "case"
                    | "do"
                    | "else"
                    | "yield"
                    | "await"
            )
        }
        Kind::Num | Kind::Str | Kind::Template => false,
        Kind::Punct => !matches!(tok_val(src, p), ")" | "]" | "++" | "--"),
    }
}

// ─────────────────────────── 结构匹配 ───────────────────────────

/// 从 `(` token 起平衡括号，返回匹配 `)` 的下标；未闭合返回 `None`。
fn match_paren(toks: &[Tok], src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = open;
    while j < toks.len() {
        if toks[j].kind == Kind::Punct {
            match tok_val(src, &toks[j]) {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        j += 1;
    }
    None
}

/// 从 `{` token 起平衡花括号，返回匹配 `}` 的下标；未闭合返回 `None`。
fn match_brace(toks: &[Tok], src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = open;
    while j < toks.len() {
        if toks[j].kind == Kind::Punct {
            match tok_val(src, &toks[j]) {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        j += 1;
    }
    None
}

/// 从 `start` 起找语句结束：深度为零的 `;`（含其自身）或深度为零的 `}`
/// （块边界，不含其自身）。返回 (token 下标, 是否含该 token)。
fn find_stmt_end(toks: &[Tok], src: &str, start: usize) -> Option<(usize, bool)> {
    let mut depth = 0usize;
    let mut j = start;
    while j < toks.len() {
        if toks[j].kind == Kind::Punct {
            match tok_val(src, &toks[j]) {
                "(" | "[" | "{" => depth += 1,
                // 右括号在深度为零时出现（畸形输入），饱和减保持深度为零，
                // 扫描继续到末尾后安全退出，不回绕也不 panic。
                ")" | "]" => depth = depth.saturating_sub(1),
                "}" if depth == 0 => return Some((j, false)),
                "}" => depth -= 1,
                ";" if depth == 0 => return Some((j, true)),
                _ => {}
            }
        }
        j += 1;
    }
    None
}

// ─────────────────────────── 严格性 ───────────────────────────

/// 判断源码是否含顶层 `import`/`export` 声明（深度为零的语句起始位）：
/// 命中即 ES 模块（恒严格）。动态 `import(x)` 非语句起始位，不命中。
fn has_top_level_module_syntax(toks: &[Tok], src: &str) -> bool {
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == Kind::Punct {
            match tok_val(src, t) {
                "{" | "(" | "[" => depth += 1,
                "}" | ")" | "]" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth == 0
            && t.kind == Kind::Ident
            && is_stmt_start(toks, src, i)
            && matches!(tok_val(src, t), "import" | "export")
        {
            return true;
        }
        i += 1;
    }
    false
}

/// 判断以 `stmt_tok` 为首个 token 的语句是否为 `"use strict"` 指令。
fn is_directive_strict(toks: &[Tok], src: &str, stmt_tok: usize) -> bool {
    let Some(t) = toks.get(stmt_tok) else {
        return false;
    };
    if t.kind != Kind::Str {
        return false;
    }
    // 去掉首尾引号后逐字比对（指令值无转义）。
    let v = &src[t.start + 1..t.end - 1];
    v == "use strict"
}

/// 收集严格函数体区间（函数体首语句为 "use strict" 指令）：`function` 声明/
/// 表达式、箭头函数块体与对象/类方法体。返回字节区间列表，区间内不改写。
fn collect_strict_regions(toks: &[Tok], src: &str) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut i = 0usize;
    while i < toks.len() {
        let t = &toks[i];
        let v = tok_val(src, t);

        if t.kind == Kind::Ident && v == "function" {
            if let Some(open) = function_body_open(toks, src, i) {
                if let Some(close) = match_brace(toks, src, open) {
                    if is_directive_strict(toks, src, open + 1) {
                        regions.push((toks[open].start, toks[close].end));
                    }
                }
            }
        }

        // 对象/类方法体：名字后（可选 `*`）紧跟参数括号与 `{`。
        // `get`/`set`/`async`/`static` 前缀落在名字之前，锚定名字即覆盖。
        if t.kind == Kind::Ident {
            if let Some(open) = method_body_open(toks, src, i) {
                if let Some(close) = match_brace(toks, src, open) {
                    if is_directive_strict(toks, src, open + 1) {
                        regions.push((toks[open].start, toks[close].end));
                    }
                }
            }
        }

        // 箭头函数块体：=> 后直接跟 {
        if t.kind == Kind::Punct && v == "=>" {
            if let Some(next) = toks.get(i + 1) {
                if next.kind == Kind::Punct && tok_val(src, next) == "{" {
                    if let Some(close) = match_brace(toks, src, i + 1) {
                        if is_directive_strict(toks, src, i + 2) {
                            regions.push((next.start, toks[close].end));
                        }
                    }
                }
            }
        }

        i += 1;
    }
    regions
}

/// 从 `function` token 起定位函数体的 `{` 下标：跳过可选函数名与 `*`，
/// 平衡参数括号后须紧跟 `{`。星号可紧邻关键字（`function* name`）也可紧邻
/// 参数括号（`function name*`），两种顺序都跳过。
fn function_body_open(toks: &[Tok], src: &str, i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j < toks.len() && toks[j].kind == Kind::Punct && tok_val(src, &toks[j]) == "*" {
        j += 1;
    }
    if j < toks.len() && toks[j].kind == Kind::Ident {
        j += 1;
        if j < toks.len() && toks[j].kind == Kind::Punct && tok_val(src, &toks[j]) == "*" {
            j += 1;
        }
    }
    if j >= toks.len() || toks[j].kind != Kind::Punct || tok_val(src, &toks[j]) != "(" {
        return None;
    }
    let close = match_paren(toks, src, j)?;
    let k = close + 1;
    if k < toks.len() && toks[k].kind == Kind::Punct && tok_val(src, &toks[k]) == "{" {
        return Some(k);
    }
    None
}

/// 从方法名 token 起定位方法体的 `{` 下标：跳过可选 `*`（生成器方法），
/// 平衡参数括号后须紧跟 `{`。调用表达式后不合法地跟块，此形态只会命中
/// 对象/类方法定义（或非法源，安全方向同函数臂）。
fn method_body_open(toks: &[Tok], src: &str, i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j < toks.len() && toks[j].kind == Kind::Punct && tok_val(src, &toks[j]) == "*" {
        j += 1;
    }
    if j >= toks.len() || toks[j].kind != Kind::Punct || tok_val(src, &toks[j]) != "(" {
        return None;
    }
    let close = match_paren(toks, src, j)?;
    let k = close + 1;
    if k < toks.len() && toks[k].kind == Kind::Punct && tok_val(src, &toks[k]) == "{" {
        return Some(k);
    }
    None
}

// ─────────────────────────── 改写扫描 ───────────────────────────

/// 语句起始判定：前一显著 token 为 `;`、`{`、`}` 或源码起点。
fn is_stmt_start(toks: &[Tok], src: &str, i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let p = &toks[i - 1];
    if p.kind != Kind::Punct {
        return false;
    }
    matches!(tok_val(src, p), ";" | "{" | "}")
}

/// for 语句起始：普通语句起始，或标签前缀 `Ident : for`。
fn is_for_stmt_start(toks: &[Tok], src: &str, i: usize) -> bool {
    if is_stmt_start(toks, src, i) {
        return true;
    }
    i >= 2 && toks[i - 1].kind == Kind::Punct && tok_val(src, &toks[i - 1]) == ":" && toks[i - 2].kind == Kind::Ident
}

fn in_strict_region(pos: usize, regions: &[(usize, usize)]) -> bool {
    regions.iter().any(|(s, e)| *s <= pos && pos < *e)
}

/// 扫描全部语句起始位，收集改写区间 (起始字节, 结束字节, 替换文本)。
fn scan_rewrites(toks: &[Tok], src: &str, strict_regions: &[(usize, usize)]) -> Vec<(usize, usize, String)> {
    let mut rewrites = Vec::new();
    let mut i = 0usize;

    while i < toks.len() {
        let t = &toks[i];
        let v = tok_val(src, t);
        let strict = in_strict_region(t.start, strict_regions);

        // for (IDENT ( ARGS ) in/of …) 整句改写（标签前缀 L: for 亦为语句起始）
        if t.kind == Kind::Ident && v == "for" && !strict && is_for_stmt_start(toks, src, i) {
            if let Some((start, end, next, repl)) = try_for_stmt(toks, src, i) {
                rewrites.push((start, end, repl));
                i = next;
                continue;
            }
        }

        if !is_stmt_start(toks, src, i) || strict {
            i += 1;
            continue;
        }

        // ++IDENT ( ARGS ) / --IDENT ( ARGS ) 前缀更新
        if t.kind == Kind::Punct && (v == "++" || v == "--") {
            if let Some((start, end, next, repl)) = try_prefix_update(toks, src, i) {
                rewrites.push((start, end, repl));
                i = next;
                continue;
            }
        }

        // IDENT ( ARGS ) 后接赋值/更新运算符
        if t.kind == Kind::Ident {
            if let Some((start, end, next, repl)) = try_call_assign(toks, src, i) {
                rewrites.push((start, end, repl));
                i = next;
                continue;
            }
        }

        i += 1;
    }

    rewrites
}

/// 判定 `i` 处的 `IDENT ( ARGS )` 后接赋值/更新运算符并收集整句改写。
/// 逻辑赋值 `&&=` `||=` `??=` 与比较/箭头等 `=` 形运算符不改写。
fn try_call_assign(toks: &[Tok], src: &str, i: usize) -> Option<(usize, usize, usize, String)> {
    let name = &toks[i];
    let p = toks.get(i + 1)?;
    if p.kind != Kind::Punct || tok_val(src, p) != "(" {
        return None;
    }
    let close = match_paren(toks, src, i + 1)?;

    let op = toks.get(close + 1)?;
    if op.kind != Kind::Punct {
        return None;
    }
    let opv = tok_val(src, op);
    let is_target = matches!(
        opv,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "**=" | "<<=" | ">>=" | ">>>=" | "&=" | "^=" | "|=" | "++" | "--"
    );
    if !is_target {
        return None;
    }

    let (end_tok, include) = find_stmt_end(toks, src, close + 1)?;
    let end = if include { toks[end_tok].end } else { toks[end_tok].start };
    let call = &src[name.start..toks[close].end];
    // 后接 `++`/`--` 为更新表达式，用 increment/decrement 文案；其余为赋值。
    let msg = if matches!(opv, "++" | "--") {
        "Invalid left-hand side in increment/decrement"
    } else {
        "Invalid left-hand side in assignment"
    };
    let repl = format!("{call}; throw new ReferenceError(\"{msg}\");");
    Some((name.start, end, end_tok + 1, repl))
}

/// 判定 `i` 处的 `++`/`--` 后接 `IDENT ( ARGS )`（前缀更新）并收集整句改写。
fn try_prefix_update(toks: &[Tok], src: &str, i: usize) -> Option<(usize, usize, usize, String)> {
    let name = toks.get(i + 1)?;
    if name.kind != Kind::Ident {
        return None;
    }
    let p = toks.get(i + 2)?;
    if p.kind != Kind::Punct || tok_val(src, p) != "(" {
        return None;
    }
    let close = match_paren(toks, src, i + 2)?;

    let (end_tok, include) = find_stmt_end(toks, src, close + 1)?;
    let end = if include { toks[end_tok].end } else { toks[end_tok].start };
    let call = &src[name.start..toks[close].end];
    let repl = format!("{call}; throw new ReferenceError(\"Invalid left-hand side in increment/decrement\");");
    Some((toks[i].start, end, end_tok + 1, repl))
}

/// 判定 `i` 处的 `for (IDENT ( ARGS ) in/of …)` 并收集整句改写（含标签前缀）。
fn try_for_stmt(toks: &[Tok], src: &str, i: usize) -> Option<(usize, usize, usize, String)> {
    let mut j = i + 1;
    if j < toks.len() && toks[j].kind == Kind::Ident && tok_val(src, &toks[j]) == "await" {
        j += 1;
    }
    let for_open = toks.get(j)?;
    if for_open.kind != Kind::Punct || tok_val(src, for_open) != "(" {
        return None;
    }
    let for_close = match_paren(toks, src, j)?;

    // 头部：IDENT ( ARGS ) in/of
    let name = toks.get(j + 1)?;
    if name.kind != Kind::Ident {
        return None;
    }
    let p = toks.get(j + 2)?;
    if p.kind != Kind::Punct || tok_val(src, p) != "(" {
        return None;
    }
    let lhs_close = match_paren(toks, src, j + 2)?;
    let kw = toks.get(lhs_close + 1)?;
    if kw.kind != Kind::Ident || !matches!(tok_val(src, kw), "in" | "of") {
        return None;
    }

    // 语句体：块体平衡花括号；非块体找语句结束
    let body = toks.get(for_close + 1)?;
    let (end_tok, include) = if body.kind == Kind::Punct && tok_val(src, body) == "{" {
        let close = match_brace(toks, src, for_close + 1)?;
        (close, true)
    } else {
        find_stmt_end(toks, src, for_close + 1)?
    };
    let end = if include { toks[end_tok].end } else { toks[end_tok].start };

    // 标签前缀：L: for(…) → L: { … }（块语句保标签合法）
    let call = &src[name.start..toks[lhs_close].end];
    let inner = format!("{call}; throw new ReferenceError(\"Invalid left-hand side in for-in loop\");");
    let (start, repl) = if i >= 2
        && toks[i - 1].kind == Kind::Punct
        && tok_val(src, &toks[i - 1]) == ":"
        && toks[i - 2].kind == Kind::Ident
    {
        let label = tok_val(src, &toks[i - 2]);
        (toks[i - 2].start, format!("{label}: {{ {inner} }}"))
    } else {
        (toks[i].start, inner)
    };

    Some((start, end, end_tok + 1, repl))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(s: &str) -> String {
        rewrite_call_lhs(s).into_owned()
    }

    // 七种目标形式各一

    #[test]
    fn simple_assignment() {
        let out = rw("function() { f() = g(); }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in assignment\");"));
        assert!(!out.contains("= g()"));
    }

    #[test]
    fn compound_assignment() {
        let out = rw("function() { f() += g(); }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in assignment\");"));
        assert!(!out.contains("+= g()"));
    }

    #[test]
    fn postfix_update() {
        let out = rw("function() { f()++; }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in increment/decrement\");"));
    }

    #[test]
    fn prefix_update() {
        let out = rw("function() { ++f(); }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in increment/decrement\");"));
    }

    #[test]
    fn for_in() {
        let out = rw("function() { for (f() in [1]) {} }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in for-in loop\");"));
        assert!(!out.contains("for (f()"));
    }

    #[test]
    fn for_of() {
        let out = rw("function() { for (f() of [1]) {} }");
        assert!(out.contains("f(); throw new ReferenceError(\"Invalid left-hand side in for-in loop\");"));
        assert!(!out.contains("for (f()"));
    }

    #[test]
    fn async_named() {
        let out = rw("function async() {}\nfunction() { async() = 1; }");
        assert!(out.contains("async(); throw new ReferenceError(\"Invalid left-hand side in assignment\");"));
    }

    // 回退形式（不改写，原样透传）

    #[test]
    fn parenthesized_not_rewritten() {
        let src = "function() { (f()) = 1; }";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn member_call_not_rewritten() {
        let src = "function() { a.b() = c; }";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn strict_top_level_not_rewritten() {
        let src = "\"use strict\";\nf() = 1;";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn logical_assignment_not_rewritten() {
        let src = "f() &&= 1;";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn arrow_body_not_rewritten() {
        let src = "a => b() = c;";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn plain_call_not_rewritten() {
        let src = "f();";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn equality_not_rewritten() {
        let src = "f() == 1;";
        assert_eq!(rw(src), src);
    }

    #[test]
    fn template_literal_not_rewritten() {
        let src = "f()`` = 1;";
        assert_eq!(rw(src), src);
    }

    // 畸形输入（语句未闭合多一个右括号）原样透传，不 panic 也不改写
    #[test]
    fn unbalanced_stmt_end_not_rewritten() {
        let src = "f() = 1)";
        assert_eq!(rw(src), src);
    }
}

#[cfg(test)]
mod probe {
    use super::*;
    fn rw(s: &str) -> String {
        rewrite_call_lhs(s).into_owned()
    }
    // 模块源（顶层 import）不改写
    #[test]
    fn module_not_rewritten() {
        let src = "import \"x\";\nf() = g();";
        assert_eq!(rw(src), src);
    }
    // 括号形式 for-in 不改写
    #[test]
    fn paren_for_in_not_rewritten() {
        let src = "for ((a=b) in {x:1}) {}";
        assert_eq!(rw(src), src);
    }
    // strict 函数体内不改写
    #[test]
    fn strict_fn_body_not_rewritten() {
        let src = "function() { \"use strict\"; f() = g(); }";
        assert_eq!(rw(src), src);
    }
    // strict 具名生成器体（星号在函数名之前）不改写
    #[test]
    fn strict_named_generator_body_not_rewritten() {
        let src = "function* g() { \"use strict\"; f() = g(); }";
        assert_eq!(rw(src), src);
    }
    // strict 对象方法体不改写
    #[test]
    fn strict_object_method_body_not_rewritten() {
        let src = "o = { m() { \"use strict\"; f() = g(); } };";
        assert_eq!(rw(src), src);
    }
    // strict 类方法体不改写
    #[test]
    fn strict_class_method_body_not_rewritten() {
        let src = "class C { m() { \"use strict\"; f() = g(); } }";
        assert_eq!(rw(src), src);
    }
}
