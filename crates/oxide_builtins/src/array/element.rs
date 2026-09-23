//! Array 元素变更与查找方法（push/pop/slice/splice/concat/join/indexOf 等）。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

use oxide_types::private_key::{make_well_known_symbol_key, WELL_KNOWN_SYMBOL_IS_CONCAT_SPREADABLE};

use super::common::{
    array_ptr, array_type_error, arraylike_get, arraylike_get_or_err, arraylike_index_present, check_array_create_len,
    clamp_relative, get_this_array_ref, get_this_arraylike, js_array_index, string_arraylike_units,
    to_integer_or_infinity_bounded, unit_string_value,
};
use super::from::{
    array_from_set_length, array_from_set_prop, array_species_create, create_data_property_or_throw, from_engine_error,
};

/// 按规范读单个元素：HasProperty 门控（自身与原型链任一层命中才算存在，
/// 数组元素区 hole 视同缺失），命中时返回 Get 值（用户 getter 的异常原样
/// 上抛），缺失返回 `None`。
fn gated_get<H: VmHost>(
    vm: &mut H, arr_ptr: *mut JsObject, index: usize, recv: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let key_si = vm.string_key_si(&index.to_string());
    if !vm.has_property(unsafe { &*arr_ptr }, key_si) {
        return Ok(None);
    }
    let val = match vm.ordinary_get(unsafe { &*arr_ptr }, key_si, recv) {
        Ok(v) => v,
        Err(msg) => return Err(from_engine_error(vm, &msg)),
    };
    Ok(Some(val))
}

/// DeletePropertyOrThrow：自有不可配置属性删除失败抛 TypeError，缺失位空操作
/// 成功。数组元素区与命名属性双路径交由 delete_own_property_outcome 判定。
fn delete_prop_or_throw<H: VmHost>(vm: &mut H, obj: &mut JsObject, key_si: u32) -> Result<(), JsValue> {
    match crate::object::delete_own_property_outcome(vm, obj, key_si) {
        crate::object::DeleteOutcome::NonConfigurable => Err(array_type_error(vm, "Cannot delete property")),
        _ => Ok(()),
    }
}

/// `Array.prototype.push(...items)`：追加元素到尾部，返回新长度。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根；长度按规范 `ToLength(Get(O, "length"))`
///    读取（上限 2^53-1）。
/// 2. `len + 参数数 > 2^53-1` 抛 TypeError（"Invalid array length"）。
/// 3. 每个实参按当前索引生成属性键，以严格模式 `[[Set]]` 写入（继承 setter /
///    不可写元素按规范抛 TypeError），写入成功后才递增索引。
/// 4. 以 `Set(this, "length", len, true)` 收尾，length 不可写时抛 TypeError。
///
/// # 副作用
/// - 修改元素区与 length；元素写入可经原型链触发用户 setter。
pub fn array_push<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.push called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 元素 Set 用户窗口须保持 GC 根。call 转发形态的实参寄存器集
    // 可占该槽，占位时跳过钉位（装箱体按接收者值传递保活）。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, mut len, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let n_items = args.len().saturating_sub(1);
    // len + 参数数超 2^53-1 抛 TypeError（node 消息 "Invalid array length"）。
    if len + n_items > 9_007_199_254_740_991 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Invalid array length"));
    }
    for &arg_reg in args.iter().skip(1) {
        let key = vm.string_key_si(&len.to_string());
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, key, vm.reg(arg_reg), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        len += 1;
    }

    // length 收尾同样走 Set：不可写 length 在此抛 TypeError。
    let length_si = vm.string_key_si("length");
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(len), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(js_array_index(len))
}

/// `Array.prototype.pop()`：移除并返回末位元素；空数组返回 undefined。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根；长度按规范 `ToLength(Get(O, "length"))`
///    读取（上限 2^53-1）。
/// 2. 长度为 0 时以 `Set(this, "length", +0, true)` 收尾返回 undefined。
/// 3. 否则 Get 末位元素（钉入返回寄存器跨后续窗口保 GC 根），
///    DeletePropertyOrThrow 末位（不可配置自有索引抛 TypeError），
///    以 `Set(this, "length", len-1, true)` 收尾，返回末位元素。
///
/// # 副作用
/// - 删除末位元素并收缩 length；Get 可触发原型链 getter，length 不可写时抛 TypeError。
pub fn array_pop<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.pop called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter 用户窗口须保持 GC 根；末位元素钉入后此槽让位，其后装箱体
    // 按接收者值传递保活。call 转发形态的实参寄存器集可占该槽，占位时跳过钉位。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, len, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let length_si = vm.string_key_si("length");
    if len == 0 {
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, JsValue::int(0), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        return NativeResult::Ok(JsValue::undefined());
    }
    let index = len - 1;
    let key = vm.string_key_si(&index.to_string());
    let last = match vm.ordinary_get(unsafe { &*arr_ptr }, key, recv) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };

    // 末位元素可能来自原型链 getter 的全新临时对象：钉入返回寄存器
    // 跨 delete / Set length 用户窗口保 GC 根，收尾自钉位读回。
    vm.set_reg(0, last);
    // DeletePropertyOrThrow：不可配置自有索引删除失败抛 TypeError，hole / 缺失位空操作。
    if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, key) {
        return NativeResult::Err(err);
    }
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(index), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(vm.reg(0))
}

/// `Array.prototype.slice(start, end)`：复制区间元素返回新数组（支持负索引）。
pub fn array_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.slice called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let n_f = n as f64;
    // ToIntegerOrInfinity(start)，缺省 0；对象经 ToPrimitive(number)。
    let rel_start = if args.len() > 1 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    // 规范：end 为 undefined 时使用 len；其余走 ToIntegerOrInfinity。
    let rel_end = if args.len() > 2 && !vm.reg(args[2]).is_undefined() {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        n_f
    };
    let start = clamp_relative(rel_start, n_f, n);
    let end = clamp_relative(rel_end, n_f, n);
    let count = end.saturating_sub(start);
    if let Err(err) = check_array_create_len(vm, count) {
        return NativeResult::Err(err);
    }
    // ArraySpeciesCreate(O, count)：真数组按 constructor/@@species 构造目标。
    let a_ptr = match array_species_create(vm, this_val, is_array, count) {
        Ok(p) => p,
        Err(err) => return NativeResult::Err(err),
    };
    // HasProperty + Get + CreateDataPropertyOrThrow 逐元素拷贝；洞位只在真数组
    // 目标上留洞（目标预填 present-undefined，须显式置洞）。
    let mut k = start;
    let mut out_idx = 0usize;
    let mut holes: Vec<usize> = Vec::new();
    while k < end {
        if arraylike_index_present(vm, arr_ptr, k) {
            let elem = arraylike_get_or_err!(vm, arr_ptr, k);
            let out_key = vm.new_string(&out_idx.to_string());
            let out_key_si = vm.property_key_si(out_key);
            if let Err(err) = create_data_property_or_throw(vm, unsafe { &mut *a_ptr }, out_key_si, elem) {
                return NativeResult::Err(err);
            }
        } else {
            holes.push(out_idx);
        }
        out_idx += 1;
        k += 1;
    }
    // 收尾 Set(A, "length", final)：终长 = 区间长度（end - start），洞位计入；
    // 普通 Set 语义（可触发继承 setter/异常）。
    let a_val = JsValue::from_js_object(a_ptr);
    if unsafe { &*a_ptr }.is_array() {
        for p in holes {
            unsafe {
                (*a_ptr).mark_hole_at(p);
            }
        }
    }
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    if let Err(err) = vm.ordinary_set(unsafe { &mut *a_ptr }, length_si, js_array_index(count), a_val, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(JsValue::from_js_object(a_ptr))
}

/// `Array.prototype.splice(start, deleteCount, ...items)`：删除并/或插入元素，
/// 返回被删除元素组成的新数组。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根（removed 数组钉入后让位；返回值为 removed
///    数组，非装箱体）；长度按规范 `ToLength(Get(O, "length"))` 读取
///    （上限 2^53-1）。
/// 2. `start` / `deleteCount` 走 ToIntegerOrInfinity 折算（负数自尾部、夹到
///    界内）；无实参时删除数恒 0，仅给 start 时删到尾部。新长度超 2^53-1
///    抛 TypeError。
/// 3. 先按 ArraySpeciesCreate 建 removed 数组（收集与搬移之前，用户构造器
///    读到未改动的 O），arraylike 路径无 species 语义但 ArrayCreate 自身
///    要求长度 ≤ 2^32-1；结果对象钉入返回寄存器（其后的收集/搬移/插入各带
///    用户调用窗口，Rust 局部指针不属 GC 根集）。
/// 4. 逐被删下标 HasProperty 门控 + Get，命中 CreateDataPropertyOrThrow；
///    缺失位仅真数组 A 保洞（A 预填 present-undefined 须显式置洞），非数组
///    A 按规范不建属性。
/// 5. `Set(A, "length", deleteCount, true)` 在搬移前收尾。
/// 6. 双向搬移：源存在则 Get + 严格 Set 到目标，源缺失则目标 DeletePropertyOrThrow
///    （不可配置抛 TypeError）；扩容方向下超出当前元素数的目标先记录，length
///    写入后补置洞；收缩方向的尾部区间自 length 写入前的语义由长度截断
///    覆盖，arraylike 上按 DeletePropertyOrThrow 逐位降序删除。
/// 7. 插入项逐个严格 Set，`Set(this, "length", newLen, true)` 收尾（length
///    不可写抛 TypeError）。
///
/// # 副作用
/// - 元素被删/插/移；Get/Set 可经原型链触发用户代码。
pub fn array_splice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.splice called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 折算 / 构造器读用户窗口须保持 GC 根。返回寄存器是调用方
    // 跨 native 调用不保活值的唯一槽（其余槽位调用方帧可能持活值，钉入即覆写）；
    // call 转发形态的实参寄存器集可占该槽，占位时跳过钉位（装箱体按接收者值
    // 传递保活）。removed 数组钉入后此槽让位，其后窗口装箱体按接收者值传递保活。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let n_f = n as f64;

    let insert_count = if args.len() > 3 { args.len() - 3 } else { 0 };
    let rel_start = if args.len() > 1 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    let start = clamp_relative(rel_start, n_f, n);
    // 无实参时删除数恒 0；deleteCount 缺省（仅有 start）即删到尾部（len - start）。
    let delete_count = if args.len() == 1 {
        0
    } else if args.len() > 2 {
        let rel_dc = match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        };
        if rel_dc < 0.0 {
            0
        } else {
            rel_dc.min(n_f - start as f64) as usize
        }
    } else {
        n - start
    };

    // 新长度超 2^53-1 抛 TypeError（node 消息 "Invalid array length"）。
    let new_len = n - delete_count + insert_count;
    if new_len > 9_007_199_254_740_991 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Invalid array length"));
    }

    // removed 先行按 ArraySpeciesCreate 构建（收集/搬移之前）并钉入返回寄存器：
    // 搬移/插入窗口的用户 setter 可触发回收，结果对象指针须保 GC 根，返回时
    // 从钉位读回；species 读与构造器抛错即 splice 抛错。
    if let Err(err) = check_array_create_len(vm, delete_count) {
        return NativeResult::Err(err);
    }
    let removed_arr = match array_species_create(vm, recv, is_array, delete_count) {
        Ok(p) => p,
        Err(err) => return NativeResult::Err(err),
    };
    vm.set_reg(0, JsValue::from_js_object(removed_arr));
    for k in 0..delete_count {
        match gated_get(vm, arr_ptr, start + k, recv) {
            Ok(Some(val)) => {
                let key_si = vm.string_key_si(&k.to_string());
                if let Err(err) = create_data_property_or_throw(vm, unsafe { &mut *removed_arr }, key_si, val) {
                    return NativeResult::Err(err);
                }
            }
            // 源缺失位仅真数组 A 保洞（A 预填 present-undefined，须显式置洞）；
            // 非数组 A 不建属性。
            Ok(None) => {
                if unsafe { &*removed_arr }.is_array() {
                    unsafe { (*removed_arr).mark_hole_at(k) }
                }
            }
            Err(err) => return NativeResult::Err(err),
        }
    }

    // Set(A, "length", actualDeleteCount, true) 先于搬移：真数组 A 构造器已置
    // 长度时为空写，构造器忽略单参时扩长。
    let a_val = JsValue::from_js_object(removed_arr);
    let length_si = vm.string_key_si("length");
    if let Err(err) =
        vm.ordinary_set(unsafe { &mut *removed_arr }, length_si, js_array_index(delete_count), a_val, true)
    {
        return NativeResult::Err(from_engine_error(vm, &err));
    }

    // 目标超出当前元素数的洞位无法即时置（越界 no-op），length 写入后补。
    let mut pending_holes: Vec<usize> = Vec::new();

    if insert_count > delete_count {
        let shift = insert_count - delete_count;
        for i in (start + delete_count..n).rev() {
            let to = i + shift;
            match gated_get(vm, arr_ptr, i, recv) {
                Ok(Some(val)) => {
                    let to_si = vm.string_key_si(&to.to_string());
                    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to_si, val, recv, true) {
                        return NativeResult::Err(from_engine_error(vm, &err));
                    }
                }
                Ok(None) => {
                    let to_si = vm.string_key_si(&to.to_string());
                    if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, to_si) {
                        return NativeResult::Err(err);
                    }
                    // 越界目标在真数组上被 ordinary_set 自动扩成
                    // present-undefined，length 写入后须补洞；arraylike 无元素区
                    // 扩展，属性即 present，无洞可补。
                    if is_array && to >= n {
                        pending_holes.push(to);
                    }
                }
                Err(err) => return NativeResult::Err(err),
            }
        }
    } else if insert_count < delete_count {
        let shift = delete_count - insert_count;
        for i in start + delete_count..n {
            let to = i - shift;
            match gated_get(vm, arr_ptr, i, recv) {
                Ok(Some(val)) => {
                    let to_si = vm.string_key_si(&to.to_string());
                    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to_si, val, recv, true) {
                        return NativeResult::Err(from_engine_error(vm, &err));
                    }
                }
                Ok(None) => {
                    let to_si = vm.string_key_si(&to.to_string());
                    if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, to_si) {
                        return NativeResult::Err(err);
                    }
                }
                Err(err) => return NativeResult::Err(err),
            }
        }
        // 收缩尾部 [new_len, n) 降序 DeletePropertyOrThrow：真数组由 length 写入的
        // 截断与不可配置阻挡扫描覆盖（重复删除是纯开销），arraylike 的 length
        // 写不触及索引属性，必须显式删位。
        if !is_array {
            for i in (new_len..n).rev() {
                let key_si = vm.string_key_si(&i.to_string());
                if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, key_si) {
                    return NativeResult::Err(err);
                }
            }
        }
    }

    for i in 0..insert_count {
        let to_si = vm.string_key_si(&(start + i).to_string());
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to_si, vm.reg(args[3 + i]), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
    }

    // Set(this, "length", newLen, true) 同样走 Set：length 不可写在此抛
    // TypeError（与新旧长度是否相同无关）。
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(new_len), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    for h in pending_holes {
        unsafe { (*arr_ptr).mark_hole_at(h) };
    }

    // 返回被删元素组成的 removed 数组（自返回槽读回）；装箱体钉位仅跨窗口保根，
    // 不作返回值。
    NativeResult::Ok(vm.reg(0))
}

/// `Array.prototype.concat(...items)`：连接 this 与参数返回新数组。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根（结果 A 钉入后让位，装箱体此后按接收者值
///    传递保活）。
/// 2. `A = ArraySpeciesCreate(O, 0)`（真数组按 constructor/@@species 构造，
///    读取先于一切 isConcatSpreadable 读取）并钉入返回寄存器；元素写入一律
///    CreateDataPropertyOrThrow 语义（frozen 等受限目标抛 TypeError）。
/// 3. 逐源 `E ∈ [O, ...args]`：`IsConcatSpreadable(E)`——非对象 false；对象
///    读 `@@isConcatSpreadable`（getter 异常透传），值非 undefined 取
///    ToBoolean，否则取 IsArray(E)。
/// 4. spreadable 源：`len = ToLength(Get(E, "length"))`（getter/数值转换异常
///    透传）；`n + len > 2^53-1` 抛 TypeError；逐索引 HasProperty 门控 + Get，
///    命中写 `A[n+k]`，缺失位记录洞（真数组目标越界写会扩成 present-undefined，
///    length 写入后补置洞）。
/// 5. 非 spreadable 源：整值写 `A[n]`；`n += 1`。
/// 6. 收尾 `Set(A, "length", n, true)`（length 不可写抛 TypeError）。
///
/// # 边界与前提
/// - 字符串 exotic 源（装箱串）索引未物化：length 与单元直读 [[StringData]]，
///   与 Array.from 的 array-like 路径同面。
/// - 引擎密集数组上限 2^32-1（真数组源 len 天然不越界），len 越上限的展开
///   仅可能来自人造大 length 对象，写入按引擎密集封顶处理。
///
/// # 副作用
/// - species/constructor、isConcatSpreadable、length 与逐元素 Get 各可经原型链
///   触发用户代码；结果 A 钉返回寄存器，返回时自钉位读回。
pub fn array_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.concat called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在
    // 根集），跨 constructor/species 读取用户窗口须保 GC 根；结果 A 钉入后
    // 让位，其后装箱体按接收者值传递保活。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    // SAFETY: this_val 为对象值，指针指向存活对象。
    let is_array = unsafe { &*this_val.as_js_object_ptr() }.is_array();

    // 全部源值（this 在前、实参随后）先读入局部：结果钉返回寄存器可能覆写
    // 实参寄存器槽，须先于钉位完成读取。
    let mut sources: Vec<JsValue> = Vec::with_capacity(args.len());
    sources.push(this_val);
    for &reg in args.iter().skip(1) {
        sources.push(vm.reg(reg));
    }

    // ArraySpeciesCreate(O, 0)：真数组按 constructor/@@species 构造目标，
    // array-like 直接 ArrayCreate（不读 constructor）。
    let a_ptr = match array_species_create(vm, this_val, is_array, 0) {
        Ok(p) => p,
        Err(err) => return NativeResult::Err(err),
    };
    // SAFETY: a_ptr 来自 ArraySpeciesCreate，指向存活对象。
    let a_is_array = unsafe { &*a_ptr }.is_array();
    // 结果钉入返回寄存器：其后的元素读取 / 写入窗口保 GC 根，返回时自钉位读回。
    vm.set_reg(0, JsValue::from_js_object(a_ptr));

    let spread_key = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_IS_CONCAT_SPREADABLE);
    let length_si = vm.string_key_si("length");
    let mut n_acc: u64 = 0;
    let mut holes: Vec<usize> = Vec::new();

    // 源按序展开。
    for e_val in sources.iter().copied() {
        // IsConcatSpreadable：非对象 false；对象读 @@isConcatSpreadable（getter
        // 异常透传），值非 undefined 取 ToBoolean，否则取 IsArray(E)。
        let e_ptr = e_val.as_js_object_ptr();
        let spreadable = if !e_val.is_object() || e_ptr.is_null() {
            false
        } else {
            // SAFETY: e_ptr 来自对象值，指向存活对象。
            let v = match vm.ordinary_get(unsafe { &*e_ptr }, spread_key, e_val) {
                Ok(v) => v,
                Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
            };
            if v.is_undefined() {
                // SAFETY: 同上。
                unsafe { &*e_ptr }.is_array()
            } else {
                oxide_runtime_api::to_boolean(v)
            }
        };
        if !spreadable {
            // 非 spreadable 源：整值追加。
            if let Err(err) = array_from_set_prop(vm, a_ptr, a_is_array, n_acc as usize, e_val) {
                return NativeResult::Err(err);
            }
            n_acc += 1;
            continue;
        }
        // 字符串 exotic 源（装箱串）单元直读（与 Array.from 同面）：length 即
        // 单元数，其余源 len = ToLength(Get(E, "length"))，getter 与数值转换
        // 异常透传。
        let string_units = string_arraylike_units(vm, e_val);
        let len = match &string_units {
            Some(units) => units.len(),
            None => {
                let len_val = match vm.ordinary_get(unsafe { &*e_ptr }, length_si, e_val) {
                    Ok(v) => v,
                    Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
                };
                if len_val.is_symbol() {
                    return NativeResult::Err(array_type_error(vm, "Cannot convert a Symbol value to a number"));
                }
                let len_num = match vm.coerce_number_bounded(len_val) {
                    Ok(v) => v,
                    Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
                };
                if len_num.is_nan() || len_num <= 0.0 {
                    0usize
                } else {
                    len_num.min(9_007_199_254_740_991.0) as usize
                }
            }
        };
        // n + len > 2^53-1 抛 TypeError（f64 比较免整数上溢）。
        if n_acc as f64 + len as f64 > 9_007_199_254_740_991.0 {
            return NativeResult::Err(array_type_error(vm, "Invalid array length"));
        }
        for k in 0..len {
            let elem = match &string_units {
                Some(units) => {
                    if k < units.len() {
                        Some(unit_string_value(vm, units[k]))
                    } else {
                        None
                    }
                }
                None => match gated_get(vm, e_ptr, k, e_val) {
                    Ok(v) => v,
                    Err(err) => return NativeResult::Err(err),
                },
            };
            match elem {
                Some(val) => {
                    if let Err(err) = array_from_set_prop(vm, a_ptr, a_is_array, (n_acc as usize) + k, val) {
                        return NativeResult::Err(err);
                    }
                }
                // 缺失位：真数组目标越界写扩成 present-undefined，length 写入后
                // 补置洞；非数组目标不建属性。
                None => holes.push((n_acc as usize) + k),
            }
        }
        n_acc += len as u64;
    }

    // 收尾 Set(A, "length", n, true)：length 不可写抛 TypeError。
    if let Err(err) = array_from_set_length(vm, a_ptr, n_acc as usize) {
        return NativeResult::Err(err);
    }
    // 缺失位在 length 写入后补洞（真数组元素区由 length 写扩展，落洞方在界内）。
    if a_is_array {
        for h in holes {
            // SAFETY: a_ptr 为存活结果对象，h < 终长。
            unsafe { (*a_ptr).mark_hole_at(h) };
        }
    }
    NativeResult::Ok(vm.reg(0))
}

/// `Array.prototype.join(separator)`：用分隔符连接元素字符串（null/undefined 视为空串）。
pub fn array_join<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.join called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    // 分隔符缺省或 undefined 时取 ","（规范：undefined 视同未给）。
    let sep = if args.len() > 1 && !vm.reg(args[1]).is_undefined() {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        ",".to_string()
    };
    let parts: Vec<String> = match (0..n)
        .map(|i| {
            let v = arraylike_get(vm, arr_ptr, i)?;
            if v.is_undefined() || v.is_null() {
                Ok(String::new())
            } else {
                Ok(oxide_runtime_api::to_string(v))
            }
        })
        .collect::<Result<Vec<String>, JsValue>>()
    {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let joined = parts.join(&sep);
    NativeResult::Ok(vm.new_string(&joined))
}

/// `Array.prototype.toString`：委托给 join，默认用 `,` 分隔。
pub fn array_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // 与 TA 原型共享同一函数对象：TA 臂先入口校验（detach 抛 TypeError）
    // 再按逗号连接（与 join 同口径），Array 臂委托 join。
    let this_ptr = vm.reg(args[0]).as_js_object_ptr();
    if !this_ptr.is_null() && unsafe { &*this_ptr }.is_typed_array_obj() {
        return crate::typed_array::typed_array_join(vm, &[args[0]]);
    }
    array_join(vm, &[args[0]])
}

/// `Array.prototype.indexOf(searchElement, fromIndex)`：用严格相等查找首个匹配索引，找不到返回 -1。
pub fn array_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.indexOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 || args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    // fromIndex：负索引从尾部折算，NaN 视为 0。
    let from_index = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        let f = if f.is_nan() { 0.0 } else { f.trunc() };
        if f >= 0.0 {
            (f as usize).min(n)
        } else {
            let from = n as f64 + f;
            if from < 0.0 {
                0
            } else {
                from as usize
            }
        }
    } else {
        0
    };
    for i in from_index..n {
        // 洞位不参与比较：HasProperty 缺失即跳过（洞不是 undefined 元素）。
        if !arraylike_index_present(vm, ptr, i) {
            continue;
        }
        let elem = arraylike_get_or_err!(vm, ptr, i);
        if oxide_runtime_api::strict_equality(elem, target) {
            return NativeResult::Ok(js_array_index(i));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `Array.prototype.includes(searchElement, fromIndex)`：用 SameValueZero 判断是否包含
/// （NaN 视为存在、+0/-0 视为相同）。
pub fn array_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.includes called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        let f = if f.is_nan() { 0.0 } else { f.trunc() };
        if f >= 0.0 {
            (f as usize).min(n)
        } else {
            let from = n as f64 + f;
            if from < 0.0 {
                0
            } else {
                from as usize
            }
        }
    } else {
        0
    };
    for i in from_index..n {
        let elem = arraylike_get_or_err!(vm, ptr, i);
        // SameValueZero：NaN 视为相等、+0 与 -0 视为相同。
        if oxide_runtime_api::same_value_zero(elem, target) {
            return NativeResult::Ok(JsValue::bool(true));
        }
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// `Array.prototype.reverse()`：原地反转元素顺序，返回 this。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根（交换环换值临时钉复用此槽）；长度按规范
///    `ToLength(Get(O, "length"))` 读取（上限 2^53-1）。
/// 2. 逐对交换 `[lower, upper]`（`upper = length - lower - 1`，`lower` 到
///    `floor(length/2)` 为止）：两端各自 HasProperty 门控，存在端 Get 取值。
/// 3. 双存在：两端严格 Set 互写；单存在：值 Set 到缺失端 + 存在端
///    DeletePropertyOrThrow；双缺失不动。
///
/// # 副作用
/// - 元素对调/置洞；Get/Set 可经原型链触发用户代码。
pub fn array_reverse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.reverse called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 折算用户窗口须保持 GC 根。返回寄存器是调用方跨 native
    // 调用不保活值的唯一槽；call 转发形态的实参寄存器集可占该槽，占位时跳过
    // 钉位（装箱体按接收者值传递保活）。交换环换值临时钉复用此槽，循环期装箱体
    // 按接收者值传递保活。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let middle = n / 2;
    let mut lower = 0usize;
    while lower < middle {
        let upper = n - lower - 1;
        let lower_si = vm.string_key_si(&lower.to_string());
        let upper_si = vm.string_key_si(&upper.to_string());
        // 先下后上读值（getter 副作用序对齐规范）；hole 与原型缺失判端不存在。
        let lower_val = if vm.has_property(unsafe { &*arr_ptr }, lower_si) {
            match vm.ordinary_get(unsafe { &*arr_ptr }, lower_si, recv) {
                Ok(v) => Some(v),
                Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
            }
        } else {
            None
        };
        // 下值取出即钉入返回寄存器：跨上端读与两端 Set 的用户调用窗口期间
        // 保持 GC 根，写后从钉位读回（Rust 局部不属根集）。
        if let Some(v) = lower_val {
            vm.set_reg(0, v);
        }
        let upper_val = if vm.has_property(unsafe { &*arr_ptr }, upper_si) {
            match vm.ordinary_get(unsafe { &*arr_ptr }, upper_si, recv) {
                Ok(v) => Some(v),
                Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
            }
        } else {
            None
        };
        match (lower_val, upper_val) {
            (Some(_lv), Some(ov)) => {
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, lower_si, ov, recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, upper_si, vm.reg(0), recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
            }
            (None, Some(ov)) => {
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, lower_si, ov, recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
                if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, upper_si) {
                    return NativeResult::Err(err);
                }
            }
            (Some(_lv), None) => {
                if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, lower_si) {
                    return NativeResult::Err(err);
                }
                // 上端写前记录元素区大小：读值期 getter 截断元素区后，上端写在
                // 区域外越界扩展会把 [old_count, upper) 填成 present-undefined，
                // 写后须整段补置洞（lower 落入段内时重复补置幂等）；仅真数组
                // 有元素区扩展。
                let old_count = unsafe { &*arr_ptr }.prop_count() as usize;
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, upper_si, vm.reg(0), recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
                if is_array {
                    for h in old_count..upper {
                        unsafe { (*arr_ptr).mark_hole_at(h) };
                    }
                }
            }
            (None, None) => {}
        }
        lower += 1;
    }
    NativeResult::Ok(this_val)
}

/// `Array.prototype.flat([depth])`：按深度递归展开嵌套数组返回新数组（循环引用原样保留）。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根（结果数组钉入后让位，装箱体按接收者值传递保活）；
///    长度按规范 `ToLength(Get(O, "length"))` 读取（上限 2^53-1）。
/// 2. depth：无实参/undefined 实参缺省 1；否则 ToIntegerOrInfinity（转换异常
///    传播），负数归 0 不展开，+∞ 保留无限深度臂。
/// 3. 结果 `A` 按 `ArraySpeciesCreate(O, 0)` 构建（真数组按 constructor/@@species
///    构造，非可构造值抛 TypeError）并钉入返回寄存器；写入一律
///    CreateDataPropertyOrThrow（frozen 等受限目标抛 TypeError），真数组结果的
///    length 随元素写入逐位扩展。
/// 4. 流式 DFS：显式栈帧 = (源对象, 源长度, 剩余深度, 下一索引)；每个源索引
///    HasProperty 门控、命中才 Get；深度内的数组元素压栈递归（长度按
///    LengthOfArrayLike 读取），已在栈上的数组（循环引用）原样写入结果；洞位
///    不写目标（结果恒紧凑）。
///
/// # 副作用
/// - 接收者/元素的 Get 可经原型链触发用户代码；species 构造与目标属性写入
///   可按规范抛 TypeError。
pub fn array_flat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.flat called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，跨 length getter / depth 折算
    // 用户窗口须保 GC 根。call 转发形态的实参寄存器集可占该槽，占位时跳过钉位
    // （装箱体按接收者值传递保活）。结果 A 钉入后此槽让位。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    // depth：缺省 1；ToIntegerOrInfinity 后负数归 0（NaN 折算为 0 同不展开），
    // +∞ 保留无限深度。
    let mut depth = 1.0f64;
    if args.len() > 1 && !vm.reg(args[1]).is_undefined() {
        depth = match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        };
        if depth < 0.0 {
            depth = 0.0;
        }
    }
    // ArraySpeciesCreate(O, 0)：真数组按 constructor/@@species 构造目标。
    let a_ptr = match array_species_create(vm, this_val, is_array, 0) {
        Ok(p) => p,
        Err(err) => return NativeResult::Err(err),
    };
    // 结果钉入返回寄存器：其后的元素 Get / 嵌套长度 Get 各带用户窗口，结果
    // 指针须保 GC 根，返回时从钉位读回。
    vm.set_reg(0, JsValue::from_js_object(a_ptr));

    // 流式 DFS：帧 = (源对象, 源长度, 剩余深度, 下一源索引)。
    let mut stack: Vec<(*mut JsObject, usize, f64, usize)> = vec![(arr_ptr, n, depth, 0)];
    let mut target_index = 0usize;
    while let Some(&top) = stack.last() {
        let (src, src_len, d, i) = top;
        if i >= src_len {
            stack.pop();
            continue;
        }
        // HasProperty 门控：洞位不写目标（结果恒紧凑）。
        if !arraylike_index_present(vm, src, i) {
            if let Some(top) = stack.last_mut() {
                top.3 = i + 1;
            }
            continue;
        }
        let elem = arraylike_get_or_err!(vm, src, i);
        // 深度内的数组元素压栈递归；已在栈上的数组（循环引用）原样写入。
        let e_ptr = if elem.is_object() { elem.as_js_object_ptr() } else { std::ptr::null_mut() };
        let descend = d > 0.0
            && !e_ptr.is_null()
            && unsafe { &*e_ptr }.is_array()
            && !stack.iter().any(|f| std::ptr::eq(f.0, e_ptr));
        if let Some(top) = stack.last_mut() {
            top.3 = i + 1;
        }
        if descend {
            // elementLen = LengthOfArrayLike(element)。
            let (_, elen, _) = match get_this_arraylike(vm, elem) {
                Ok(v) => v,
                Err(e) => return NativeResult::Err(e),
            };
            stack.push((e_ptr, elen, d - 1.0, 0));
            continue;
        }
        // 目标索引上限 2^53-1（规范 FlattenIntoArray 步 6.vi.i）。
        if target_index >= 9_007_199_254_740_991 {
            return NativeResult::Err(array_type_error(vm, "Invalid array length"));
        }
        let key = vm.new_string(&target_index.to_string());
        let key_si = vm.property_key_si(key);
        if let Err(err) = create_data_property_or_throw(vm, unsafe { &mut *a_ptr }, key_si, elem) {
            return NativeResult::Err(err);
        }
        target_index += 1;
    }
    NativeResult::Ok(JsValue::from_js_object(a_ptr))
}

/// `Array.prototype.shift()`：移除并返回首元素，其余元素前移；空数组返回 undefined。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根；长度按规范 `ToLength(Get(O, "length"))`
///    读取（上限 2^53-1）。
/// 2. 长度为 0 时以 `Set(this, "length", +0, true)` 收尾返回 undefined。
/// 3. Get 首元素并钉入返回寄存器；`1..len` 逐位前移：HasProperty 命中
///    （自身与原型链，hole 视同缺失）则 Get + 严格 Set 到前一格，缺失则
///    DeletePropertyOrThrow 目标格。
/// 4. 末位 DeletePropertyOrThrow 后以 `Set(this, "length", len-1, true)` 收尾。
///
/// # 副作用
/// - 元素整体前移并收缩 length；Get/Set 可经原型链触发用户代码。
pub fn array_shift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.shift called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter 用户窗口须保持 GC 根；首位元素钉入后此槽让位，其后装箱体
    // 按接收者值传递保活。call 转发形态的实参寄存器集可占该槽，占位时跳过钉位。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, len, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let length_si = vm.string_key_si("length");
    if len == 0 {
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, JsValue::int(0), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        return NativeResult::Ok(JsValue::undefined());
    }
    let first_key = vm.string_key_si("0");
    let first = match vm.ordinary_get(unsafe { &*arr_ptr }, first_key, recv) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 首位为 hole 时 Get 落原型链 getter，返回值可能是全新临时对象。钉入结果寄存器
    // （GC 根）再跨循环：循环内用户 setter 可触发回收，Rust 局部不属根集，回收后
    // 返回即悬垂；重入调用保存/恢复调用方窗口，槽值至返回前有效。
    vm.set_reg(0, first);

    for k in 1..len {
        let to = vm.string_key_si(&(k - 1).to_string());
        // HasProperty 走原型链存在性判定（hole 视同缺失），命中才 Get + Set。
        match gated_get(vm, arr_ptr, k, recv) {
            Ok(Some(val)) => {
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to, val, recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
            }
            Ok(None) => {
                // DeletePropertyOrThrow(O, to)：不可配置元素删除失败抛 TypeError。
                if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, to) {
                    return NativeResult::Err(err);
                }
            }
            Err(err) => return NativeResult::Err(err),
        }
    }

    // 末位 DeletePropertyOrThrow：真数组上该位已由 length 写入截断覆盖，
    // arraylike 上该位不在搬移路径内，须显式删除。
    let last_key = vm.string_key_si(&(len - 1).to_string());
    if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, last_key) {
        return NativeResult::Err(err);
    }
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(len - 1), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    // 从钉位读回（回收搬移后局部副本已失效）。
    NativeResult::Ok(vm.reg(0))
}

/// `Array.prototype.unshift(...items)`：插入元素到头部，返回新长度。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根；长度按规范 `ToLength(Get(O, "length"))`
///    读取（上限 2^53-1）。
/// 2. 参数数大于 0 且 `len + 参数数 > 2^53-1` 抛 TypeError（现行规范文本，
///    非 RangeError）。
/// 3. 参数数大于 0 时自高到低搬移已有元素：HasProperty 命中（自身与原型链，
///    hole 视同缺失）则 Get + 严格 Set（搬移值钉入返回寄存器跨 Set 窗口保
///    GC 根），缺失则 DeletePropertyOrThrow，再把实参逐个严格 Set 到头部。
/// 4. 以 `Set(this, "length", len+参数数, true)` 收尾。
///
/// # 副作用
/// - 元素整体后移并扩容 length；Get/Set 可经原型链触发用户代码。
pub fn array_unshift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.unshift called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 搬移窗口用户窗口须保持 GC 根；搬移值钉入后此槽让位，
    // 其后装箱体按接收者值传递保活。call 转发形态的实参寄存器集可占该槽，
    // 占位时跳过钉位。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, len, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let recv = this_val;
    let n_items = args.len().saturating_sub(1);
    let length_si = vm.string_key_si("length");
    // 实参值先读入局部：搬移循环把搬移值钉入返回寄存器，可能覆写实参寄存器
    // 槽，读取须先于钉位完成。
    let arg_values: Vec<JsValue> = args.iter().skip(1).map(|&r| vm.reg(r)).collect();
    if n_items > 0 {
        // len + 参数数超 2^53-1 抛 TypeError（规范现行文本，node 实测同形）。
        if len + n_items > 9_007_199_254_740_991 {
            return NativeResult::Err(crate::error::create_type_error(vm, "Invalid array length"));
        }
        for k in (1..=len).rev() {
            let to_idx = k - 1 + n_items;
            let to = vm.string_key_si(&to_idx.to_string());
            match gated_get(vm, arr_ptr, k - 1, recv) {
                Ok(Some(val)) => {
                    // 搬移值跨 Set 用户调用窗口：先钉返回寄存器再写，写后从钉位读回。
                    vm.set_reg(0, val);
                    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to, vm.reg(0), recv, true) {
                        return NativeResult::Err(from_engine_error(vm, &err));
                    }
                }
                Ok(None) => {
                    if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, to) {
                        return NativeResult::Err(err);
                    }
                }
                Err(err) => return NativeResult::Err(err),
            }
        }
        for (j, val) in arg_values.iter().enumerate() {
            let key = vm.string_key_si(&j.to_string());
            if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, key, *val, recv, true) {
                return NativeResult::Err(from_engine_error(vm, &err));
            }
        }
    }

    let new_len = len + n_items;
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(new_len), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(js_array_index(new_len))
}

/// `Array.prototype.fill(value, start, end)`：用给定值填充区间，返回 this。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根；长度按规范 `ToLength(Get(O, "length"))`
///    读取（上限 2^53-1）。
/// 2. `start` / `end` 走 ToIntegerOrInfinity 折算（负数自尾部、夹到界内）；
///    `end` 为 undefined 取 len。
/// 3. 区间内每格无条件严格 `Set`（无 HasProperty 门控，洞位物化为 present）。
///
/// # 副作用
/// - 区间元素被覆写；Set 可经原型链触发用户 setter / 继承 setter。
pub fn array_fill<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.fill called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 折算 / 元素 Set 用户窗口须保持 GC 根。返回寄存器是调用方
    // 跨 native 调用不保活值的唯一槽，本方法全程独占，收尾自钉位读回；call 转发
    // 形态的实参寄存器集可占该槽，占位时跳过钉位。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let n_f = n as f64;
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let rel_start = if args.len() > 2 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    // 规范：end 为 undefined 时取 len，其余走 ToIntegerOrInfinity。
    let rel_end = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[3])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        n_f
    };
    let start = clamp_relative(rel_start, n_f, n);
    let end = clamp_relative(rel_end, n_f, n);
    for i in start..end {
        let key_si = vm.string_key_si(&i.to_string());
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, key_si, value, this_val, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
    }
    NativeResult::Ok(this_val)
}

/// `Array.prototype.copyWithin(target, start, end)`：在数组内部复制元素区间，
/// 返回 this。
///
/// # 步骤
/// 1. `ToObject` 装箱基元 this（null/undefined 抛 TypeError），装箱体钉入返回
///    寄存器跨用户窗口保 GC 根（复制环复制值临时钉复用此槽）；长度按规范
///    `ToLength(Get(O, "length"))` 读取（上限 2^53-1）。
/// 2. `target` / `start` / `end` 走 ToIntegerOrInfinity 折算（负数自尾部、夹到
///    [0, length]）；`end` 为 undefined 取 len；`count = min(end - start,
///    length - target)`。
/// 3. 源区间与目标重叠且目标领先时反向迭代（否则正向复制会先覆盖源），
///    其余正向。
/// 4. 逐位：源 HasProperty 命中则 Get + 严格 Set 到目标，缺失则目标
///    DeletePropertyOrThrow（不可配置抛 TypeError）。
///
/// # 副作用
/// - 元素被覆写/置洞；Get/Set 可经原型链触发用户代码。
pub fn array_copy_within<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.copyWithin called with {} args", args.len());
    // ToObject：基元 this 装箱（null/undefined 抛 TypeError）。
    let this_val = match oxide_runtime_api::to_object(vm.reg(args[0]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // 装箱体钉入返回寄存器：它只存于 Rust 局部，this 寄存器持原始基元（不在根集），
    // 跨 length getter / 折算用户窗口须保持 GC 根。返回寄存器是调用方跨 native
    // 调用不保活值的唯一槽；call 转发形态的实参寄存器集可占该槽，占位时跳过
    // 钉位（装箱体按接收者值传递保活）。复制环复制值临时钉复用此槽，循环期装箱体
    // 按接收者值传递保活。
    if !args.contains(&0) {
        vm.set_reg(0, this_val);
    }
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    // 空数组同样完成三个索引的 ToIntegerOrInfinity（转换期异常/副作用须
    // 先于空区间短路）。
    let n_f = n as f64;
    let rel_target = if args.len() > 1 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    let rel_start = if args.len() > 2 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    // 规范：end 为 undefined 时取 len，其余走 ToIntegerOrInfinity。
    let rel_end = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[3])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        n_f
    };
    let target = clamp_relative(rel_target, n_f, n);
    let start = clamp_relative(rel_start, n_f, n);
    let end = clamp_relative(rel_end, n_f, n);
    // end < start 时区间为空（saturating 免下溢）。
    let count = end.saturating_sub(start).min(n - target);
    if count == 0 {
        return NativeResult::Ok(this_val);
    }
    let (mut from, mut to, backward) = if start < target && target < start + count {
        (start + count - 1, target + count - 1, true)
    } else {
        (start, target, false)
    };
    for _ in 0..count {
        match gated_get(vm, arr_ptr, from, this_val) {
            Ok(Some(v)) => {
                // 复制值跨 Set 用户调用窗口：先钉返回寄存器再写，写后从钉位读回。
                vm.set_reg(0, v);
                let to_si = vm.string_key_si(&to.to_string());
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to_si, vm.reg(0), this_val, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
            }
            Ok(None) => {
                let to_si = vm.string_key_si(&to.to_string());
                if let Err(err) = delete_prop_or_throw(vm, unsafe { &mut *arr_ptr }, to_si) {
                    return NativeResult::Err(err);
                }
            }
            Err(err) => return NativeResult::Err(err),
        }
        // 末轮迭代后的步进值不再被读取，saturating 免基元 0 下溢。
        if backward {
            from = from.saturating_sub(1);
            to = to.saturating_sub(1);
        } else {
            from += 1;
            to += 1;
        }
    }
    NativeResult::Ok(this_val)
}

/// `Array.prototype.at(index)`：按索引取元素，支持负索引；越界返回 undefined。
/// 长度取逻辑长度（含超出 dense 元素数的稀疏覆盖值），负索引自它折算；
/// 元素读走规范 Get（洞位落原型链，原型索引访问器触发），密集数组直读快路径。
pub fn array_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.at called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let len = unsafe { &*arr_ptr }.logical_len() as u64;
    // ToIntegerOrInfinity：NaN/±0 折算 0，符号化转换异常（如 Symbol）原样上抛。
    let rel = if args.len() > 1 {
        match vm.coerce_number_bounded(vm.reg(args[1])) {
            Ok(v) => v,
            Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
        }
    } else {
        0.0
    };
    let rel = if rel.is_nan() || rel == 0.0 { 0.0 } else { rel.trunc() };
    let index = if rel < 0.0 {
        // k = len + relative 在 64 位整型域折算：极负 rel 饱和为 i64::MIN 时 k 必 < 0
        //（规范 k < 0 返 undefined），仅 rel ∈ [-len, 0) 落入界内下标；浮点域钳零会
        // 把越界负索引静默映射回首元素。
        let k = (len as i64) + (rel as i64);
        if k < 0 {
            return NativeResult::Ok(JsValue::undefined());
        }
        k as u64
    } else {
        rel.min(len as f64) as u64
    };
    if index >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    NativeResult::Ok(arraylike_get_or_err!(vm, arr_ptr, index as usize))
}

/// `Array.prototype.lastIndexOf(searchElement, fromIndex)`：从后往前查找首个匹配索引。
pub fn array_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.lastIndexOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let search = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // fromIndex（缺省：n-1）。
    let from_index_isize: isize = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        if f.is_nan() {
            return NativeResult::Ok(JsValue::int(-1));
        }
        let f = f.trunc();
        if f >= 0.0 {
            (f as isize).min(n as isize - 1)
        } else {
            n as isize + f as isize
        }
    } else {
        n as isize - 1
    };
    if from_index_isize < 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    for i in (0..=from_index_isize as usize).rev() {
        // 洞位不参与比较：HasProperty 缺失即跳过（洞不是 undefined 元素）。
        if !arraylike_index_present(vm, ptr, i) {
            continue;
        }
        let elem = arraylike_get_or_err!(vm, ptr, i);
        if oxide_runtime_api::strict_equality(elem, search) {
            return NativeResult::Ok(js_array_index(i));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}
