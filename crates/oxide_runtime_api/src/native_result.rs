//! builtin native 函数的返回值：`Ok`/`Err`/`TailCall` 三态，含构造与转换辅助。

use oxide_types::value::JsValue;

/// 每个 builtin native 函数的返回值。
pub enum NativeResult {
    Ok(JsValue),
    Err(JsValue),
    TailCall { callee: JsValue, this: JsValue, args: Vec<JsValue> },
}

impl NativeResult {
    /// 构造成功结果，携带一个 `JsValue` 返回值。
    pub fn ok(val: JsValue) -> Self {
        Self::Ok(val)
    }

    /// 构造失败结果，携带被抛出的异常值。
    pub fn err(val: JsValue) -> Self {
        Self::Err(val)
    }

    /// 取出成功值；若为 `Err` 或 `TailCall` 则 panic。仅用于已知必然成功的场景。
    pub fn unwrap(self) -> JsValue {
        match self {
            Self::Ok(val) => val,
            Self::Err(_) => panic!("called `NativeResult::unwrap()` on an `Err` value"),
            Self::TailCall { .. } => panic!("called `NativeResult::unwrap()` on a `TailCall` value"),
        }
    }

    /// 把 `Err` 分支的错误值映射为自定义错误类型并转为 `Result`；
    /// `TailCall` 不能转换，遇到时 panic。
    pub fn map_err<E, F>(self, op: F) -> Result<JsValue, E>
    where
        F: FnOnce(JsValue) -> E,
    {
        match self {
            Self::Ok(val) => Ok(val),
            Self::Err(err) => Err(op(err)),
            Self::TailCall { .. } => panic!("TailCall cannot be converted to Result"),
        }
    }
}
