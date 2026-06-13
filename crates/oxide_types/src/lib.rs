#![doc = "OxideJS - Shared core types (JsValue, JsObject, Shape, P, Epoch)"]

pub mod error;
pub mod mem;
pub mod object;
pub mod shape;
pub mod value;

pub use error::{JsError, JsErrorKind};
