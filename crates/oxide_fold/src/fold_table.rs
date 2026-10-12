//! 折叠表：一元与算术常量折叠（A 组）、位运算与比较折叠（C 组）加逻辑短路
//! 选支（B 组）。
//!
//! 折值编码逐运算符镜像运行时产物编码，保证物化后与运行时执行结果逐字节
//! 一致：折值计算复用 `oxide_runtime_api::coercion` 的纯函数族（`to_number` /
//! `to_int32` / `to_uint32` / `to_boolean` / `relational_compare` /
//! `abstract_eq_primitives` / `strict_equality`），与 emit 层字面量折叠同源，
//! 杜绝两层折叠各自镜像运行时语义的漂移风险。
//!
//! 语义守卫：
//! - 算术二元（ADD/SUB/MUL/DIV/MOD/EXP）任一操作数非数值不折；BigInt 混合
//!   （一 BigInt 一非 BigInt）不折（运行期抛 TypeError，BigInt 与 BigInt 有
//!   独立指令族）；SUB/MUL/DIV/MOD/EXP 的 BigInt 操作数不折（折值恒升 f64，
//!   无法表示 BigInt 结果）；DIV/MOD 除数为零不折、EXP 指数为负不折（保持
//!   运行时指令为除零/负指数编码的唯一来源）。
//! - ADD 的 int+int 经 `checked_add` 在 i32 内保 `Int`，溢出升 `Number`
//!   （镜像运行时 `int_add`）；其余组合 f64 加（不做恒等捷径，负零操作数
//!   经实加得 `+0.0`）。
//! - UNARY_PLUS 作用 BigInt 不折（运行期抛 TypeError）。
//! - C 组位运算与比较：双操作数须均在 `Int`/`Number`/`Boolean`/`Null` 四变体
//!   域，`BigInt`/`String`/`Undefined` 操作数不折（与共享表 `const_to_jsvalue`
//!   四臂口径一致，字符串比较路径降为后续任务）。
//!
//! B 组逻辑短路：结果恒为某一操作数的原值（无类型转换，任何类型安全），
//! 仅左操作数已知即可选支，右操作数类型不限。

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_runtime_api::{
    abstract_eq_primitives, relational_compare, strict_equality, to_boolean, to_int32, to_number, to_uint32,
};
use oxide_types::string_forge::encode_key;
use oxide_types::value::JsValue;

/// 常量转 `JsValue`（纯 NaN-box 构造，零分配）：`Int`/`Number`/`Boolean`/
/// `Null`/`Undefined` 五臂；`BigInt`/`String` 是引用形态（指针载荷），不在此
/// 转换，由调用方按常量类型分派处理。
fn const_to_jsvalue(c: &Constant) -> Option<JsValue> {
    match c {
        Constant::Int(i) => Some(JsValue::int(*i)),
        Constant::Number(d) => Some(JsValue::float(*d)),
        Constant::Boolean(b) => Some(JsValue::bool(*b)),
        Constant::Null => Some(JsValue::null()),
        Constant::Undefined => Some(JsValue::undefined()),
        Constant::BigInt(_) | Constant::String(_) => None,
    }
}

/// 常量的数值（`Int`/`Number` 臂，调用方已守卫非 BigInt 数值）。
fn const_to_number(c: &Constant) -> f64 {
    to_number(const_to_jsvalue(c).expect("数值常量（非 BigInt）必有 JsValue 形态"))
}

/// 是否为数值常量（`Int`/`Number`/`BigInt` 三臂）。
fn is_numeric(c: &Constant) -> bool {
    matches!(c, Constant::Int(_) | Constant::Number(_) | Constant::BigInt(_))
}

/// 是否为 BigInt 常量。
fn is_bigint(c: &Constant) -> bool {
    matches!(c, Constant::BigInt(_))
}

/// 常量的布尔值（`ToBoolean` 口径）：BigInt 臂判零、字符串臂判非空（池键形态
/// 空串即空串），其余经 `const_to_jsvalue` 加 `to_boolean`。
fn is_truthy(c: &Constant) -> bool {
    match c {
        Constant::BigInt(b) => *b != num_bigint::BigInt::from(0),
        Constant::String(s) => !s.is_empty(),
        _ => to_boolean(const_to_jsvalue(c).expect("非 BigInt/String 常量必有 JsValue 形态")),
    }
}

/// 常量是否为 nullish（`Null`/`Undefined` 两臂）。
fn is_nullish(c: &Constant) -> bool {
    matches!(c, Constant::Null | Constant::Undefined)
}

/// A 组一元折叠：按操作码计算折值，守卫命中返回 `None`。
///
/// 折值编码逐运算符镜像运行时产物：
/// - `VOID`：恒 `Undefined`（操作数副作用已在发射期为独立指令，本指令恒装 undefined）
/// - `NOT`：`Boolean`（`ToBoolean` 取反，任意常量类型安全）
/// - `TYPEOF`：`String`（池键形态，逐类型镜像运行时 `TYPEOF_TEXTS` 分派：
///   Int/Number 归 "number"、BigInt 归 "bigint"、Boolean 归 "boolean"、
///   Null 归 "object"、String 归 "string"、Undefined 归 "undefined"）
/// - `NEG`：BigInt 取负保 `BigInt`；其余数值恒 `Number`（NEG 恒升 double，
///   不保 `Int`）；非数值不折
/// - `UNARY_PLUS`：`Int`/`Number` 恒 `Number`（`ToNumber`）；BigInt 不折
///   （运行期抛 TypeError）
/// - `BIT_NOT`：BigInt 按位取反保 `BigInt`；其余数值恒 `Int`（`ToInt32` 取反）
pub fn fold_unary(op: OpCode, c: &Constant) -> Option<Constant> {
    match op {
        OpCode::VOID => Some(Constant::Undefined),
        OpCode::NOT => Some(Constant::Boolean(!is_truthy(c))),
        OpCode::TYPEOF => {
            let text = match c {
                Constant::Int(_) | Constant::Number(_) => "number",
                Constant::BigInt(_) => "bigint",
                Constant::Boolean(_) => "boolean",
                Constant::Null => "object",
                Constant::String(_) => "string",
                Constant::Undefined => "undefined",
            };
            let units: Vec<u16> = text.encode_utf16().collect();
            Some(Constant::String(encode_key(&units)))
        }
        OpCode::NEG => match c {
            Constant::BigInt(b) => Some(Constant::BigInt(-b.clone())),
            Constant::Int(i) => Some(Constant::Number(-(*i as f64))),
            Constant::Number(d) => Some(Constant::Number(-*d)),
            _ => None,
        },
        OpCode::UNARY_PLUS => match c {
            Constant::Int(i) => Some(Constant::Number(*i as f64)),
            Constant::Number(d) => Some(Constant::Number(*d)),
            _ => None,
        },
        OpCode::BIT_NOT => match c {
            Constant::BigInt(b) => Some(Constant::BigInt(!b.clone())),
            Constant::Int(_) | Constant::Number(_) => Some(Constant::Int(!to_int32(
                const_to_jsvalue(c).expect("数值常量（非 BigInt）必有 JsValue 形态"),
            ))),
            _ => None,
        },
        _ => None,
    }
}

/// A 组二元算术折叠：按操作码计算折值，守卫命中返回 `None`。
///
/// 折值编码逐运算符镜像运行时产物：
/// - `ADD`：int+int 经 `checked_add` 在 i32 内保 `Int`，溢出升 `Number`
///   （镜像 `int_add`）；BigInt+BigInt 精确加；其余组合 f64 加（实加，
///   不做恒等捷径）
/// - `SUB`/`MUL`/`DIV`/`MOD`：恒 `Number` 的 f64 运算（镜像 `binary_arith!`
///   宏的 `as f64`）；DIV/MOD 除数为零不折
/// - `EXP`：恒 `Number` 的 `powf`（镜像 `exp_result`）；指数为负不折
pub fn fold_binary(op: OpCode, l: &Constant, r: &Constant) -> Option<Constant> {
    // 两操作数均须数值；BigInt 混合（一 BigInt 一非 BigInt）不折。
    if !is_numeric(l) || !is_numeric(r) {
        return None;
    }
    if is_bigint(l) != is_bigint(r) {
        return None;
    }
    match op {
        OpCode::ADD => {
            if is_bigint(l) {
                return match (l, r) {
                    (Constant::BigInt(a), Constant::BigInt(b)) => Some(Constant::BigInt(a + b)),
                    _ => None,
                };
            }
            // int+int：checked_add 在 i32 内保 Int，溢出升 f64（镜像 int_add）。
            if let (Constant::Int(a), Constant::Int(b)) = (l, r) {
                return a
                    .checked_add(*b)
                    .map(Constant::Int)
                    .or(Some(Constant::Number(*a as f64 + *b as f64)));
            }
            // 其余组合 f64 实加（负零操作数经实加得 +0.0，不取恒等捷径）。
            Some(Constant::Number(const_to_number(l) + const_to_number(r)))
        }
        OpCode::SUB | OpCode::MUL | OpCode::DIV | OpCode::MOD | OpCode::EXP => {
            // BigInt 操作数不折：折值恒升 f64，无法表示 BigInt 结果。
            if is_bigint(l) {
                return None;
            }
            // 除零守卫：Int(0) 与 Number(±0.0) 均判零（v == 0.0 对 -0.0 亦真）；
            // 负指数守卫：Int 与 Number 负值均判负。命中即不折，落回运行时指令。
            if (matches!(op, OpCode::DIV | OpCode::MOD) && const_to_number(r) == 0.0)
                || (op == OpCode::EXP && const_to_number(r) < 0.0)
            {
                return None;
            }
            let (ln, rn) = (const_to_number(l), const_to_number(r));
            Some(match op {
                OpCode::SUB => Constant::Number(ln - rn),
                OpCode::MUL => Constant::Number(ln * rn),
                OpCode::DIV => Constant::Number(ln / rn),
                OpCode::MOD => Constant::Number(ln % rn),
                OpCode::EXP => Constant::Number(ln.powf(rn)),
                _ => unreachable!("守卫已限定 SUB/MUL/DIV/MOD/EXP"),
            })
        }
        _ => None,
    }
}

/// B 组逻辑短路选支：左操作数已知时返回选定操作数（`true` = 左，`false` =
/// 右），左操作数未知返回 `None`。
///
/// 结果恒为某一操作数的原值（无类型转换）：
/// - `AND`：左真选右、左假选左
/// - `OR`：左真选左、左假选右
/// - `NULLISH`：左 nullish 选右、左非 nullish 选左
pub fn short_circuit(op: OpCode, l: &Constant) -> Option<bool> {
    match op {
        OpCode::AND => Some(!is_truthy(l)),
        OpCode::OR => Some(is_truthy(l)),
        OpCode::NULLISH => Some(!is_nullish(l)),
        _ => None,
    }
}

/// C 组操作数域守卫：仅 `Int`/`Number`/`Boolean`/`Null` 四变体可折。
///
/// 传播映射可承载 `BigInt`/`String`/`Undefined` 常量（来自 `LOAD_CONST`），
/// 这三类操作数不折（与共享表 `const_to_jsvalue` 四臂口径一致）。
fn is_foldable_c_operand(c: &Constant) -> bool {
    matches!(c, Constant::Int(_) | Constant::Number(_) | Constant::Boolean(_) | Constant::Null)
}

/// C 组位运算与比较折叠：按操作码计算折值，守卫命中返回 `None`。
///
/// 折值编码逐运算符镜像运行时产物：
/// - `BIT_AND`/`BIT_OR`/`BIT_XOR`：`Int`（`ToInt32` 按位与/或/异或）
/// - `SHL`/`SHR`：`Int`（`ToInt32` 左移/右移，移位量 `ToUint32` 与 0x1F）
/// - `USHR`：结果不超 i32::MAX 保 `Int`，否则升 `Number`（镜像
///   `write_ushr_result`）
/// - `LT`/`GT`/`LTE`/`GTE`：`Boolean`（镜像 `relational_compare`，GT/LTE
///   交换操作数，LTE/GTE 取反，`None` 归 false）
/// - `EQ`/`NEQ`：`Boolean`（镜像 `abstract_eq_primitives`）
/// - `STRICT_EQ`/`STRICT_NEQ`：`Boolean`（镜像 `strict_equality`）
///
/// 操作数域守卫：双操作数须均在 `Int`/`Number`/`Boolean`/`Null` 四变体域，
/// `BigInt`/`String`/`Undefined` 操作数不折（命中即返回 `None`）。
pub fn fold_c_group(op: OpCode, l: &Constant, r: &Constant) -> Option<Constant> {
    // 操作数域守卫先行：传播映射可承载 BigInt/String/Undefined 常量，
    // C 组表只接受四变体域。
    if !is_foldable_c_operand(l) || !is_foldable_c_operand(r) {
        return None;
    }
    let (lv, rv) = (const_to_jsvalue(l)?, const_to_jsvalue(r)?);
    match op {
        OpCode::BIT_AND | OpCode::BIT_OR | OpCode::BIT_XOR => {
            let (a, b) = (to_int32(lv), to_int32(rv));
            Some(match op {
                OpCode::BIT_AND => Constant::Int(a & b),
                OpCode::BIT_OR => Constant::Int(a | b),
                _ => Constant::Int(a ^ b),
            })
        }
        OpCode::SHL | OpCode::SHR => {
            let a = to_int32(lv);
            let shift = to_uint32(rv) & 0x1F;
            Some(match op {
                OpCode::SHL => Constant::Int(a << shift),
                _ => Constant::Int(a >> shift),
            })
        }
        OpCode::USHR => {
            let a = to_int32(lv) as u32;
            let shift = to_uint32(rv) & 0x1F;
            let result = a >> shift;
            // 结果不超 i32::MAX 保 Int，否则升 Number（镜像 write_ushr_result）。
            if result <= i32::MAX as u32 {
                Some(Constant::Int(result as i32))
            } else {
                Some(Constant::Number(result as f64))
            }
        }
        OpCode::LT | OpCode::GT | OpCode::LTE | OpCode::GTE => {
            // GT/LTE 交换操作数，LTE/GTE 取反（镜像运行时派发口径）。
            let (a, b) = match op {
                OpCode::GT | OpCode::LTE => (rv, lv),
                _ => (lv, rv),
            };
            let cmp = relational_compare(a, b).unwrap_or(false);
            let negate = matches!(op, OpCode::LTE | OpCode::GTE);
            Some(Constant::Boolean(if negate { !cmp } else { cmp }))
        }
        OpCode::EQ | OpCode::NEQ => {
            let eq = abstract_eq_primitives(lv, rv);
            Some(Constant::Boolean(if op == OpCode::NEQ { !eq } else { eq }))
        }
        OpCode::STRICT_EQ | OpCode::STRICT_NEQ => {
            let eq = strict_equality(lv, rv);
            Some(Constant::Boolean(if op == OpCode::STRICT_NEQ { !eq } else { eq }))
        }
        _ => None,
    }
}
