//! 常量折叠：操作数均为字面量且通过语义守卫时，编译期折成单常量，
//! 发射单条 `LOAD_CONST`。折值编码逐运算符镜像运行时指令的产物编码，
//! 保证物化后与运行时执行结果逐字节一致。
//!
//! 两个域并列：数值域（`literal_constant`/`fold_binary`，两操作数形态，
//! 守卫排除字符串与 BigInt）与字符串域（`literal_units`/`fold_string_chain`/
//! `fold_template`，Addition 链与全字面量模板，单元口径与运行期
//! `push_units_to` 逐段一致）。
//!
//! 数值域语义守卫：任一操作数为 BigInt 字面量不折（BigInt 混合运行期抛
//! TypeError，BigInt 与 BigInt 有独立指令族）；`DIV`/`MOD` 除数为零不折、
//! `EXP` 指数为负不折（保持运行时指令为除零/负指数编码的唯一来源）。
//! 字符串域语义守卫：Addition 链须至少一个字符串字面量（全数字链落回
//! 数值域与 `CONCAT_N`，两域零重叠）；模板须全部 cooked 为 Some。

use oxide_bytecode::module::Constant;
use oxide_parser::{BinaryOperator, Expression, UnaryOperator};
use oxide_runtime_api::{
    abstract_eq_primitives, bigint_to_string, js_number_to_string, relational_compare, strict_equality, to_int32,
    to_number, to_uint32,
};
use oxide_types::string_forge::{decode_key, encode_key};
use oxide_types::value::JsValue;

use crate::is_int_literal;
use crate::shared::string_pool::pool_key_marker;

/// 字面量表达式提取常量：剥括号与一元负号后，数字（`is_int_literal` 分派
/// `Int`/`Number`）、布尔、null 三类；BigInt 字面量、字符串字面量、RegExp 与
/// 非字面量一律 `None`。
///
/// 一元负号只沿 `UnaryNegation` 链剥（含嵌套负号与括号），其余一元运算符
/// （`!`/`typeof`/`void`/`~`/`+`/`delete`）终止剥壳不折。
pub(crate) fn literal_constant(expr: &Expression) -> Option<Constant> {
    let (inner, negated) = peel_negation(expr);
    match inner {
        Expression::NumericLiteral(n) => {
            let value = if negated { -n.value } else { n.value };
            if is_int_literal(value) {
                Some(Constant::Int(value as i32))
            } else {
                Some(Constant::Number(value))
            }
        }
        Expression::BooleanLiteral(b) if !negated => Some(Constant::Boolean(b.value)),
        Expression::NullLiteral(_) if !negated => Some(Constant::Null),
        _ => None,
    }
}

/// 剥括号与一元负号链：返回内层表达式与负号层数奇偶（奇数层取负）。
fn peel_negation<'a>(expr: &'a Expression<'a>) -> (&'a Expression<'a>, bool) {
    let mut e = expr;
    let mut negated = false;
    loop {
        while let Expression::ParenthesizedExpression(p) = e {
            e = &p.expression;
        }
        if let Expression::UnaryExpression(u) = e {
            if u.operator == UnaryOperator::UnaryNegation {
                negated = !negated;
                e = &u.argument;
                continue;
            }
        }
        break;
    }
    (e, negated)
}

/// 常量转 `JsValue`（纯 NaN-box 构造，零分配）：`Int`/`Number`/`Boolean`/`Null`
/// 四臂，其余臂不可达（BigInt/String/Undefined 已被守卫排除）。
fn const_to_jsvalue(c: &Constant) -> JsValue {
    match c {
        Constant::Int(i) => JsValue::int(*i),
        Constant::Number(d) => JsValue::float(*d),
        Constant::Boolean(b) => JsValue::bool(*b),
        Constant::Null => JsValue::null(),
        _ => unreachable!("折叠操作数守卫已排除 BigInt/String/Undefined"),
    }
}

/// 常量的数值（`Int`/`Number`/`Boolean`/`Null` 均走 `ToNumber` 口径）。
fn const_to_number(c: &Constant) -> f64 {
    to_number(const_to_jsvalue(c))
}

/// 双常量折叠分派：按操作码计算折值，守卫命中返回 `None`。
///
/// 折值编码逐运算符镜像运行时产物：
/// - `ADD`：int+int 经 `checked_add` 在 i32 内保 `Int`，溢出升 `Number`（镜像
///   `int_add`）；其余组合 f64 加（镜像 `dispatch_add` 的 `to_number` 路径）
/// - `SUB`/`MUL`/`DIV`/`MOD`：恒 `Number`（镜像 `binary_arith!` 的 `as f64`）
/// - `EXP`：恒 `Number` 的 `powf`（镜像 `exp_result`）
/// - `BIT_AND`/`BIT_OR`/`BIT_XOR`/`SHL`/`SHR`：`Int`（镜像 `to_int32`）
/// - `USHR`：结果 ≤ i32::MAX 保 `Int`，否则升 `Number`（镜像 `write_ushr_result`）
/// - `LT`/`GT`/`LTE`/`GTE`：`Boolean`（镜像 `relational_compare`，GT/LTE 交换
///   操作数，LTE/GTE 取反，`None` 归 false）
/// - `EQ`/`NEQ`：`Boolean`（镜像 `abstract_eq` 原始值核心）；
///   `STRICT_EQ`/`STRICT_NEQ`：`Boolean`（镜像 `strict_equality`）
///
/// 守卫：`DIV`/`MOD` 除数为零（`Int(0)` 与 `Number(±0.0)` 均判零）、`EXP`
/// 指数为负（`Int`/`Number` 负值均判负）返回 `None`，落回运行时路径。
pub(crate) fn fold_binary(op: BinaryOperator, l: &Constant, r: &Constant) -> Option<Constant> {
    match op {
        BinaryOperator::Addition => {
            // int+int：checked_add 在 i32 内保 Int，溢出升 f64（镜像 int_add）。
            if let (Constant::Int(a), Constant::Int(b)) = (l, r) {
                return a
                    .checked_add(*b)
                    .map(Constant::Int)
                    .or(Some(Constant::Number(*a as f64 + *b as f64)));
            }
            Some(Constant::Number(const_to_number(l) + const_to_number(r)))
        }
        BinaryOperator::Subtraction
        | BinaryOperator::Multiplication
        | BinaryOperator::Division
        | BinaryOperator::Remainder => {
            // 除零守卫：Int(0) 与 Number(±0.0) 均判零（v == 0.0 对 -0.0 亦真）。
            if matches!(op, BinaryOperator::Division | BinaryOperator::Remainder) && const_to_number(r) == 0.0 {
                return None;
            }
            // 恒 f64（镜像 binary_arith! 宏的 as f64）。
            let (ln, rn) = (const_to_number(l), const_to_number(r));
            Some(match op {
                BinaryOperator::Subtraction => Constant::Number(ln - rn),
                BinaryOperator::Multiplication => Constant::Number(ln * rn),
                BinaryOperator::Division => Constant::Number(ln / rn),
                _ => Constant::Number(ln % rn),
            })
        }
        BinaryOperator::Exponential => {
            // 负指数守卫：Int 与 Number 负值均判负。
            if const_to_number(r) < 0.0 {
                return None;
            }
            // 恒 f64 powf（镜像 exp_result）。
            Some(Constant::Number(const_to_number(l).powf(const_to_number(r))))
        }
        BinaryOperator::BitwiseAnd | BinaryOperator::BitwiseOR | BinaryOperator::BitwiseXOR => {
            let (a, b) = (to_int32(const_to_jsvalue(l)), to_int32(const_to_jsvalue(r)));
            Some(match op {
                BinaryOperator::BitwiseAnd => Constant::Int(a & b),
                BinaryOperator::BitwiseOR => Constant::Int(a | b),
                _ => Constant::Int(a ^ b),
            })
        }
        BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight => {
            let a = to_int32(const_to_jsvalue(l));
            let shift = to_uint32(const_to_jsvalue(r)) & 0x1F;
            Some(match op {
                BinaryOperator::ShiftLeft => Constant::Int(a << shift),
                _ => Constant::Int(a >> shift),
            })
        }
        BinaryOperator::ShiftRightZeroFill => {
            let a = to_int32(const_to_jsvalue(l)) as u32;
            let shift = to_uint32(const_to_jsvalue(r)) & 0x1F;
            let result = a >> shift;
            // 结果 ≤ i32::MAX 保 Int，否则升 Number（镜像 write_ushr_result）。
            if result <= i32::MAX as u32 {
                Some(Constant::Int(result as i32))
            } else {
                Some(Constant::Number(result as f64))
            }
        }
        BinaryOperator::LessThan => relational(l, r, false, false),
        BinaryOperator::GreaterThan => relational(l, r, true, false),
        BinaryOperator::LessEqualThan => relational(l, r, true, true),
        BinaryOperator::GreaterEqualThan => relational(l, r, false, true),
        BinaryOperator::Equality => {
            Some(Constant::Boolean(abstract_eq_primitives(const_to_jsvalue(l), const_to_jsvalue(r))))
        }
        BinaryOperator::Inequality => {
            Some(Constant::Boolean(!abstract_eq_primitives(const_to_jsvalue(l), const_to_jsvalue(r))))
        }
        BinaryOperator::StrictEquality => {
            Some(Constant::Boolean(strict_equality(const_to_jsvalue(l), const_to_jsvalue(r))))
        }
        BinaryOperator::StrictInequality => {
            Some(Constant::Boolean(!strict_equality(const_to_jsvalue(l), const_to_jsvalue(r))))
        }
        // IN/INSTANCEOF 是对象操作，不折。
        BinaryOperator::In | BinaryOperator::Instanceof => None,
    }
}

/// 关系比较折叠：镜像运行时 dispatch 口径（GT/LTE 交换操作数，LTE/GTE 取反，
/// `None` 归 false）。
fn relational(l: &Constant, r: &Constant, swap: bool, negate: bool) -> Option<Constant> {
    let (a, b) = if swap {
        (const_to_jsvalue(r), const_to_jsvalue(l))
    } else {
        (const_to_jsvalue(l), const_to_jsvalue(r))
    };
    let cmp = relational_compare(a, b).unwrap_or(false);
    Some(Constant::Boolean(if negate { !cmp } else { cmp }))
}

/// 可折叠字面量的单元序列（剥括号与一元负号链）：字符串字面量经
/// `pool_key_marker` 加 `decode_key` 还原单元（与字面量发射器同一池键函数）；
/// 数字字面量按 `is_int_literal` 分派 i32 itoa 或 `js_number_to_string` 后
/// UTF-16 编码；BigInt 字面量解析后 `bigint_to_string`（i128 快路径失败回退
/// 十进制 `parse_bytes`，解析失败返回 `None` 不折）；布尔与 null 为
/// "true"/"false"/"null"。
///
/// 其余形态（标识符/调用/RegExp/嵌套二元等）返回 `None`。
pub(crate) fn literal_units(expr: &Expression) -> Option<Vec<u16>> {
    let (inner, negated) = peel_negation(expr);
    match inner {
        Expression::StringLiteral(s) if !negated => Some(decode_key(&pool_key_marker(&s.value, s.lone_surrogates))),
        Expression::NumericLiteral(n) => {
            let value = if negated { -n.value } else { n.value };
            let text = if is_int_literal(value) {
                (value as i32).to_string()
            } else {
                js_number_to_string(value)
            };
            Some(text.encode_utf16().collect())
        }
        Expression::BigIntLiteral(b) => {
            let value = if let Ok(v) = b.value.parse::<i128>() {
                num_bigint::BigInt::from(v)
            } else {
                num_bigint::BigInt::parse_bytes(b.value.as_bytes(), 10)?
            };
            let value = if negated { -value } else { value };
            Some(bigint_to_string(&value).encode_utf16().collect())
        }
        Expression::BooleanLiteral(b) if !negated => {
            let text = if b.value { "true" } else { "false" };
            Some(text.encode_utf16().collect())
        }
        Expression::NullLiteral(_) if !negated => Some("null".encode_utf16().collect()),
        _ => None,
    }
}

/// 判定操作数是否为字符串字面量（剥括号）：规范判定整条 `+` 链为字符串
/// 拼接的条件。
fn is_string_literal(expr: &Expression) -> bool {
    let mut e = expr;
    while let Expression::ParenthesizedExpression(p) = e {
        e = &p.expression;
    }
    matches!(e, Expression::StringLiteral(_))
}

/// Addition 链字符串域折叠：全操作数经 `literal_units` 可定值且至少一个是
/// 字符串字面量时，拼接各段单元经 `encode_key` 回池键形态，返回
/// `Constant::String`；全数字链（无字符串字面量）返回 `None` 落回 `CONCAT_N`。
pub(crate) fn fold_string_chain(operands: &[&Expression]) -> Option<Constant> {
    // 字符串域守卫置于首行：至少一个字符串字面量，全数字链留给数值域。
    if !operands.iter().any(|op| is_string_literal(op)) {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for op in operands {
        units.extend(literal_units(op)?);
    }
    Some(Constant::String(encode_key(&units)))
}

/// 模板字面量折叠：全部 quasi 的 cooked 为 Some 且全部内插表达式经
/// `literal_units` 可定值时，按 quasi 与表达式交替序拼接单元经 `encode_key`
/// 回池键形态，返回 `Constant::String`；任一 cooked 为 None（非法转义，
/// 解析期 SyntaxError 的防御路径）返回 `None` 落回 `TEMPLATE_STR`。
pub(crate) fn fold_template(tl: &oxide_parser::TemplateLiteral) -> Option<Constant> {
    // cooked 守卫置于首行：任一段为 None 时整模板不折。
    if !tl.quasis.iter().all(|q| q.value.cooked.is_some()) {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for (i, q) in tl.quasis.iter().enumerate() {
        let cooked = q.value.cooked.as_ref().expect("守卫已确认全部 cooked 为 Some");
        units.extend(decode_key(&pool_key_marker(cooked, q.lone_surrogates)));
        if let Some(expr) = tl.expressions.get(i) {
            units.extend(literal_units(expr)?);
        }
    }
    Some(Constant::String(encode_key(&units)))
}
