use oxide_types::value::JsValue;

pub enum NativeResult {
    Ok(JsValue),
    Err(JsValue),
    TailCall { callee: JsValue, this: JsValue, args: Vec<JsValue> },
}

impl NativeResult {
    pub fn ok(val: JsValue) -> Self {
        Self::Ok(val)
    }

    pub fn err(val: JsValue) -> Self {
        Self::Err(val)
    }

    pub fn into_result(self) -> Result<JsValue, JsValue> {
        match self {
            Self::Ok(val) => Ok(val),
            Self::Err(err) => Err(err),
            Self::TailCall { .. } => panic!("TailCall cannot be converted to Result"),
        }
    }

    pub fn unwrap(self) -> JsValue {
        match self {
            Self::Ok(val) => val,
            Self::Err(_) => panic!("called `NativeResult::unwrap()` on an `Err` value"),
            Self::TailCall { .. } => panic!("called `NativeResult::unwrap()` on a `TailCall` value"),
        }
    }

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

pub type NativeFn = fn(&mut crate::vm::Vm, args: &[u8]) -> NativeResult;
