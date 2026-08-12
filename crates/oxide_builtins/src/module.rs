//! 模块命名空间与求值辅助（import 实现内部用，非 JS 可见标准 API）。
//!
//! 命名约定 `__module*`：由编译器模块 prelude 发出（`lookup_or_builtin` 解析为全局槽，
//! VM 在 frame push 时从 global 对象取回 native 函数）。当前为快照式链接：
//! 依赖模块先整体求值并返回命名空间对象，导入方从中读取导出值；
//! live binding / source-phase / defer 语义留待后续轮次。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 模块命名空间导出属性描述符：不可写、可枚举、不可配置（module namespace exotic）。
fn ns_attrs() -> PropAttributes {
    PropAttributes::new(false, true, false)
}

fn type_error<H: VmHost>(vm: &mut H, msg: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, msg))
}

/// `__moduleObject()`：创建模块命名空间对象（null 原型，带 @@toStringTag）。
pub fn module_object<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let tag_si = vm.kernel_core().perm_interner().intern("@@toStringTag").0;
    let tag_val = vm.new_string("Module");
    let obj_ref = unsafe { &mut *obj };
    if let Err(e) = vm.define_data_property(
        obj_ref,
        tag_si,
        tag_val,
        PropAttributes::new(false, false, false),
    ) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// `__moduleSet(ns, name, value)`：在命名空间上定义导出属性。
pub fn module_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return type_error(vm, "__moduleSet: 3 arguments required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__moduleSet: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleSet: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let name_val = vm.reg(args[2]);
    let value = vm.reg(args[3]);
    let name_si = vm.property_key_si(name_val);
    let obj = unsafe { &mut *ns_ptr };
    if let Err(e) = vm.define_data_property(obj, name_si, value, ns_attrs()) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `__moduleGet(ns, name)`：读取命名空间导出属性。
pub fn module_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return type_error(vm, "__moduleGet: 2 arguments required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__moduleGet: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleGet: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let name_val = vm.reg(args[2]);
    let name_si = vm.property_key_si(name_val);
    let obj = unsafe { &*ns_ptr };
    // 模块命名空间 exotic Get：未导出属性抛 TypeError（而非 undefined）。
    if vm.get_own_property_slot(obj, name_si).is_none() {
        return type_error(vm, "__moduleGet: requested export is not exported");
    }
    match vm.ordinary_get(obj, name_si, ns_val) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(crate::error::create_error(vm, &e)),
    }
}

/// `__moduleLinkGet(ns, name)`：import 绑定初始化（链接期语义）。
/// 与 `__moduleGet` 的区别：缺失导出抛 SyntaxError（模块声明实例化期的
/// 绑定解析错误），而非命名空间访问的 TypeError。
pub fn module_link_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return type_error(vm, "__moduleLinkGet: 2 arguments required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__moduleLinkGet: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleLinkGet: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let name_val = vm.reg(args[2]);
    let name_si = vm.property_key_si(name_val);
    let obj = unsafe { &*ns_ptr };
    if vm.get_own_property_slot(obj, name_si).is_none() {
        return NativeResult::Err(crate::error::create_syntax_error(
            vm,
            "requested module export is not exported",
        ));
    }
    match vm.ordinary_get(obj, name_si, ns_val) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(crate::error::create_error(vm, &e)),
    }
}

/// `__moduleStar(dst, src)`：把 src 命名空间的全部导出（除 default）复制到 dst。
pub fn module_star<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return type_error(vm, "__moduleStar: 2 arguments required");
    }
    let dst_val = vm.reg(args[1]);
    let src_val = vm.reg(args[2]);
    let dst_ptr = match vm.checked_object_ptr(dst_val, "__moduleStar: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleStar: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let src_ptr = match vm.checked_object_ptr(src_val, "__moduleStar: source is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleStar: source is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let src = unsafe { &*src_ptr };
    let default_si = vm.kernel_core().perm_interner().intern("default").0;
    let keys = crate::object::walk_own_keys(vm, src);
    let dst = unsafe { &mut *dst_ptr };
    for (name_si, pos) in keys {
        if name_si == default_si {
            continue;
        }
        // export * 冲突省略：名称已被本命名空间直接导出或先前 star 占用时跳过
        // （规范 StarExportEntries 求值后，ambiguous 名称从命名空间省略）。
        if vm.get_own_property_slot(dst, name_si).is_some() {
            continue;
        }
        let value = src.get_prop_at(pos);
        if let Err(e) = vm.define_data_property(dst, name_si, value, ns_attrs()) {
            return NativeResult::Err(crate::error::create_error(vm, &e));
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `__moduleSeal(ns)`：封冻命名空间（不可扩展；toStringTag 留待后续）。
pub fn module_seal<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return type_error(vm, "__moduleSeal: 1 argument required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__moduleSeal: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleSeal: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let obj = unsafe { &mut *ns_ptr };
    obj.set_extensible(false);
    NativeResult::Ok(ns_val)
}

/// `__moduleEval(fn)`：同步执行依赖模块（fn 为编译期 CREATE_CLOSURE 的函数对象），返回其命名空间。
pub fn module_eval<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return type_error(vm, "__moduleEval: 1 argument required");
    }
    let fn_val = vm.reg(args[1]);
    if !fn_val.is_object() {
        return type_error(vm, "__moduleEval: target is not a function");
    }
    let ptr = fn_val.as_js_object_ptr();
    if ptr.is_null() || !unsafe { (*ptr).is_function() } {
        return type_error(vm, "__moduleEval: target is not a function");
    }
    match vm.call_function_sync(fn_val, JsValue::undefined(), &[]) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(crate::error::create_error(vm, &e)),
    }
}

/// `__moduleData(kind, content)`：构造数据模块命名空间（json/text；bytes 未支持）。
pub fn module_data<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return type_error(vm, "__moduleData: 2 arguments required");
    }
    let kind_val = vm.reg(args[1]);
    let content_val = vm.reg(args[2]);
    if !kind_val.is_string() || !content_val.is_string() {
        return type_error(vm, "__moduleData: kind/content must be strings");
    }
    let kind = unsafe { oxide_runtime_api::string_data(kind_val) }.to_string();
    let default_val = match kind.as_str() {
        "json" => {
            // 复用 JSON.parse 的 serde 管线：args[1] 即内容字符串。
            let parse_result = crate::json::json_parse(vm, &[args[0], args[2]]);
            match parse_result {
                NativeResult::Ok(v) => v,
                NativeResult::Err(e) => return NativeResult::Err(e),
                NativeResult::TailCall { .. } => return type_error(vm, "__moduleData: unexpected tail call"),
            }
        }
        "text" => {
            let text = unsafe { oxide_runtime_api::string_data(content_val) };
            vm.new_string(text)
        }
        _ => return type_error(vm, "__moduleData: unsupported data kind"),
    };
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let default_si = vm.kernel_core().perm_interner().intern("default").0;
    let obj_ref = unsafe { &mut *obj };
    if let Err(e) = vm.define_data_property(obj_ref, default_si, default_val, ns_attrs()) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    obj_ref.set_extensible(false);
    NativeResult::Ok(JsValue::from_js_object(obj))
}
