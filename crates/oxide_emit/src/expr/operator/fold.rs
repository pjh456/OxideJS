//! 双常量折叠：两操作数均为字面量且通过语义守卫时，编译期折成单常量，
//! 发射单条 `LOAD_CONST`。折值编码逐运算符镜像运行时指令的产物编码，
//! 保证物化后与运行时执行结果逐字节一致。
//!
//! 三条语义守卫：任一操作数为 BigInt 字面量不折（BigInt 混合运行期抛
//! TypeError，BigInt 与 BigInt 有独立指令族）；`DIV`/`MOD` 除数为零不折、
//! `EXP` 指数为负不折（保持运行时指令为除零/负指数编码的唯一来源）。
//! 字符串字面量不折（字符串域是独立任务）。

use oxide_bytecode::module::Constant;
use oxide_parser::{BinaryOperator, Expression, UnaryOperator};
use oxide_runtime_api::{
    abstract_eq_primitives, relational_compare, strict_equality, to_int32, to_number, to_uint32,
};
use oxide_types::value::JsValue;

use crate::is_int_literal;

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
