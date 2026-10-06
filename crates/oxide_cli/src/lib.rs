//! oxide_cli 库目标：暴露持久 server 子系统与完成值渲染，供集成测试跨进程
//! 引用、供二进制与 server 执行路径共用。
//!
//! 二进制目标（`src/main.rs`）承载 CLI 分派（eval / run / compile / bench /
//! REPL）；库目标暴露 server 模块与完成值渲染函数，集成测试（`tests/`）经
//! `oxide_cli::server` 调用 `run_server` 起真实 server 进程主体，eval 臂与
//! server 执行路径共用 `format_js_value` 渲染完成值。

use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_types::object::JsObject;
use oxide_types::private_key::{is_private_name_key, is_symbol_key};
use oxide_vm::vm::Vm;
use oxide_vm::JsValue;

pub mod server;

/// 完成值渲染：把 JsValue 递归渲染为可读文本（字符串加引号、Promise 按结算态
/// 展开、函数 `[Function]`、对象走 shape 链、数组按元素区）。
///
/// 二进制 eval 臂与 server 执行路径共用。
pub fn format_js_value(vm: &Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, val: JsValue) -> String {
    if val.is_string() {
        // SAFETY: val 已确认是字符串值。
        let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
        format!("\"{s}\"")
    } else if val.is_bigint() {
        format!("{}", vm.bigint_value(val))
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            // 已 settle 的 Promise 打印其结算值（便于 eval 观察微任务结果）。
            return match oxide_vm::promise::promise_settled_value(obj) {
                Some((true, v)) => format_js_value(vm, string_forge, shape_forge, v),
                Some((false, v)) => {
                    format!("Promise {{ <rejected> {} }}", format_js_value(vm, string_forge, shape_forge, v))
                }
                None => "Promise { <pending> }".to_string(),
            };
        }
        if obj.is_function() {
            "[Function]".to_string()
        } else if obj.is_array() {
            format_array(vm, string_forge, shape_forge, obj)
        } else {
            format_object(vm, string_forge, shape_forge, obj)
        }
    } else if val.is_undefined() {
        "undefined".to_string()
    } else {
        format!("{val}")
    }
}

/// 对象渲染：沿 shape 父链收集非空属性名（跳过 Symbol/私有名键），按序渲染。
fn format_object(vm: &Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, obj: &JsObject) -> String {
    let mut entries = Vec::new();
    let shape_id = obj.shape_id();
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = shape_forge.get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    let mut pos: u32 = 0;
    for id in shape_ids.iter().rev() {
        if let Some(shape) = shape_forge.get_shape(*id) {
            // 跳过 Symbol/私有名键，仅展示字符串属性名。
            if !is_symbol_key(shape.property_name) && !is_private_name_key(shape.property_name) {
                let prop_val = obj.get_prop_at(pos);
                if prop_val.is_undefined() {
                    pos += 1;
                    continue;
                }
                let name = string_forge.lookup(shape.property_name).unwrap_or_default();
                let val_str = format_js_value(vm, string_forge, shape_forge, prop_val);
                entries.push(format!("\"{name}\": {val_str}"));
            }
        }
        pos += 1;
    }
    format!("{{{}}}", entries.join(", "))
}

/// 数组渲染：按元素区长度遍历，逐元素渲染。
fn format_array(vm: &Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, obj: &JsObject) -> String {
    // 数组元素在独立元素区，命名属性区长度恒 0；元素区未分配时 get_prop_at 自然返回 undefined。
    let len = obj.array_prop_count as usize;
    let mut items = Vec::new();
    for i in 0..len {
        let val = obj.get_prop_at(i);
        items.push(format_js_value(vm, string_forge, shape_forge, val));
    }
    format!("[{}]", items.join(", "))
}
