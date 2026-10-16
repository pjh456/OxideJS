//! Intl 骨架阶段的 native 实现：`getCanonicalLocales`、十组构造器（含
//! FallbackSymbol 语义）、`supportedLocalesOf`（尚无支持域过滤）与 Locale 构造器。
//!
//! 真实格式化语义（format / formatToParts / resolvedOptions / compare 等）
//! 由后续任务填充；本模块只保证构造与元数据面符合规范。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{
    to_length, to_number_full, to_object, to_string, to_string_full, NativeResult, ProtoKind, VmHost,
};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use super::cldr;
use crate::temporal::native_try;

/// 把 `Result<_, String>` 错误文本转为异常值：优先取调用期间产生的未捕获
/// 异常（getter 抛出等），无则按文本恢复 kind 创建错误对象。
fn engine_error<H: VmHost>(vm: &mut H, text: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_from_text(vm, text))
}

/// 按元素列表构造普通数组（CreateArrayFromList 语义）。
fn make_array_from_list<H: VmHost>(vm: &mut H, items: &[JsValue]) -> JsValue {
    let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        items.len().min(oxide_types::object::MAX_DENSE_PROPS),
    ));
    for (i, item) in items.iter().enumerate() {
        // SAFETY: arr 是刚经统一入口分配的数组对象，元素区已就绪。
        unsafe {
            (*arr).set_prop_at(i, *item);
        }
    }
    // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
    unsafe {
        (*arr).set_prop_count(items.len());
    }
    JsValue::from_js_object(arr)
}

/// 单个标签的校验与规范化：结构校验（RangeError）→ 规范化。
fn canonicalize_one<H: VmHost>(vm: &mut H, s: &str) -> Result<String, JsValue> {
    if !cldr::is_structurally_valid(s) {
        return Err(crate::error::create_range_error(vm, "not a valid language tag"));
    }
    Ok(cldr::canonicalize_locale(s))
}

/// CanonicalizeLocaleList（ECMA-402 9.2.1）：字符串参数按单元素列表处理；
/// 其余 ToObject → ToLength(length) → 逐元素 HasProperty/Get（尊重 getter）→
/// 原始串去重 → 结构校验（RangeError）→ 规范化 → 规范化结果去重。
/// 非字符串/对象元素抛 TypeError。
fn canonicalize_locale_list<H: VmHost>(vm: &mut H, locales: JsValue) -> Result<Vec<String>, JsValue> {
    if locales.is_undefined() {
        return Ok(Vec::new());
    }
    // 字符串参数整体是单个 locale（规范步骤 3：包装为单元素数组），
    // 不按字符序列解释。
    if locales.is_string() {
        let s = to_string(locales);
        return Ok(vec![canonicalize_one(vm, &s)?]);
    }
    let o_val = to_object(locales, vm).map_err(|e| engine_error(vm, &e))?;
    let o_ptr = o_val.as_js_object_ptr();
    if o_ptr.is_null() {
        return Ok(Vec::new());
    }
    let o = unsafe { &*o_ptr };
    let length_si = vm.perm_intern("length");
    let length_raw = vm.ordinary_get(o, length_si, o_val).map_err(|e| engine_error(vm, &e))?;
    // LengthOfArrayLike：ToNumber 全程强制（symbol 抛 TypeError），再 ToLength。
    let length_num = to_number_full(length_raw, vm).map_err(|e| engine_error(vm, &e))?;
    let len = to_length(JsValue::float(length_num)) as usize;

    let mut raw: Vec<String> = Vec::new();
    for i in 0..len {
        let key = vm.new_string(&i.to_string());
        let key_si = vm.property_key_si(key);
        let present = vm.has_property(o, key_si).map_err(|e| engine_error(vm, &e))?;
        if !present {
            continue;
        }
        let next = vm.ordinary_get(o, key_si, o_val).map_err(|e| engine_error(vm, &e))?;
        // 元素须为字符串或对象（对象经 ToString 取标签，尊重自定义 toString）；
        // 其余类型（number/boolean/null/undefined/symbol）抛 TypeError。
        let s = if next.is_string() {
            to_string(next)
        } else if next.is_object() {
            to_string_full(next, vm).map_err(|e| engine_error(vm, &e))?
        } else {
            return Err(crate::error::create_type_error(vm, "locales must be strings"));
        };
        if !raw.contains(&s) {
            raw.push(s);
        }
    }

    let mut result: Vec<String> = Vec::new();
    for s in &raw {
        let canonical = canonicalize_one(vm, s)?;
        if !result.contains(&canonical) {
            result.push(canonical);
        }
    }
    Ok(result)
}

/// `Intl.getCanonicalLocales(locales)`：CanonicalizeLocaleList 后 CreateArrayFromList。
pub fn get_canonical_locales<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let locales = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let list = native_try!(canonicalize_locale_list(vm, locales));
    let items: Vec<JsValue> = list.iter().map(|s| vm.new_string_owned(s.clone())).collect();
    NativeResult::Ok(make_array_from_list(vm, &items))
}

/// 十组构造器的共享骨架：`new` 时 this 已按 newTarget 原型创建（返回 undefined，
/// VM 回传 this）；非 legacy 构造器（PluralRules 及以后）NewTarget 为 undefined
/// 时抛 TypeError；legacy 构造器（NumberFormat/DateTimeFormat/Collator）在既有
/// 实例上非构造调用时定义 FallbackSymbol 并返回 this，其余路径新建实例返回。
///
/// FallbackSymbol 描述固定为 "IntlLegacyConstructedSymbol"，属性值取本次调用
/// 新建的实例（规范：[[Value]]: numberFormat），描述符
/// {writable:false, enumerable:false, configurable:false}。
fn intl_ctor_impl<H: VmHost>(vm: &mut H, args: &[u8], proto_kind: ProtoKind, legacy: bool) -> NativeResult {
    let proto_ptr = vm.builtin_proto(proto_kind);
    if vm.constructing_native() {
        return NativeResult::Ok(JsValue::undefined());
    }
    if !legacy {
        return NativeResult::Err(crate::error::create_type_error(vm, "constructor requires new"));
    }
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return make_intl_instance(vm, proto_ptr);
    }
    let this_ptr = this_val.as_js_object_ptr();
    if this_ptr.is_null() {
        return make_intl_instance(vm, proto_ptr);
    }
    // this 的原型链包含本构造器原型（OrdinaryHasInstance 语义）：定义
    // FallbackSymbol 并返回 this。
    if crate::temporal::is_ctor_call(vm, args, proto_ptr) {
        let this_obj = unsafe { &mut *this_ptr };
        // 属性值取本次调用新建的实例（规范：[[Value]]: numberFormat）。
        let instance_val = match make_intl_instance(vm, proto_ptr) {
            NativeResult::Ok(v) => v,
            NativeResult::Err(e) => return NativeResult::Err(e),
            NativeResult::TailCall { .. } => unreachable!("make_intl_instance 不尾调用"),
        };
        let idx = vm.symbol_intern(Some("IntlLegacyConstructedSymbol".to_string()));
        let key = oxide_types::private_key::encode_symbol_key(vm.realm_id(), idx);
        let _ = vm.define_data_property(this_obj, key, instance_val, PropAttributes::new(false, false, false));
        return NativeResult::Ok(this_val);
    }
    make_intl_instance(vm, proto_ptr)
}

/// 新建一个以指定原型为 `[[Prototype]]` 的空实例。
fn make_intl_instance<H: VmHost>(vm: &mut H, proto_ptr: *mut JsObject) -> NativeResult {
    let proto = JsValue::from_js_object(proto_ptr);
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

macro_rules! intl_ctor_legacy {
    ($name:ident, $kind:ident) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            intl_ctor_impl(vm, args, ProtoKind::$kind, true)
        }
    };
}

macro_rules! intl_ctor {
    ($name:ident, $kind:ident) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            intl_ctor_impl(vm, args, ProtoKind::$kind, false)
        }
    };
}

// legacy 构造器：可非构造调用（FallbackSymbol 语义）。
intl_ctor_legacy!(number_format_constructor, NumberFormatProto);
intl_ctor_legacy!(date_time_format_constructor, DateTimeFormatProto);
intl_ctor_legacy!(collator_constructor, CollatorProto);
// 非 legacy 构造器：NewTarget 为 undefined 时抛 TypeError。
intl_ctor!(plural_rules_constructor, PluralRulesProto);
intl_ctor!(list_format_constructor, ListFormatProto);
intl_ctor!(relative_time_format_constructor, RelativeTimeFormatProto);
intl_ctor!(display_names_constructor, DisplayNamesProto);
intl_ctor!(segmenter_constructor, SegmenterProto);
intl_ctor!(locale_constructor, LocaleProto);
intl_ctor!(duration_format_constructor, DurationFormatProto);

/// `supportedLocalesOf(locales)`：骨架阶段只执行 CanonicalizeLocaleList，
/// 尚不做支持域过滤（过滤依赖 ICU 数据，后续任务填充）。
pub fn supported_locales_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let locales = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let list = native_try!(canonicalize_locale_list(vm, locales));
    let items: Vec<JsValue> = list.iter().map(|s| vm.new_string_owned(s.clone())).collect();
    NativeResult::Ok(make_array_from_list(vm, &items))
}

/// `Intl.supportedValuesOf(key)`：骨架阶段只做键名校验（六类白名单，
/// 其余 RangeError）；合法键返回空数组（数据层随后续任务填充）。
pub fn supported_values_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let key = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let key_str = native_try!(to_string_full(key, vm).map_err(|e| engine_error(vm, &e)));
    if !matches!(
        key_str.as_str(),
        "calendar" | "collation" | "currency" | "numberingSystem" | "timeZone" | "unit"
    ) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid key"));
    }
    NativeResult::Ok(make_array_from_list(vm, &[]))
}
