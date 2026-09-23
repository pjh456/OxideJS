//! 属性键推导与解析：JsValue 转属性键 si（整数键区间/字符串键/Symbol 键）、
//! 数组下标反解、非对象基删除与属性解析及自身属性槽查找。

use oxide_runtime_api as coercion;

use oxide_types::object::JsObject;
use oxide_types::private_key::{
    int_key_value, is_int_key, make_int_key, make_symbol_key, make_well_known_symbol_key, INT_KEY_COUNT,
    WELL_KNOWN_SYMBOL_COUNT,
};
use oxide_types::value::JsValue;

use super::{canonical_index_of, canonical_index_units, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::vm_trace;

/// 把符号原语的下标编码为属性键：well-known 下标走保留键槽，用户下标加偏移，
/// 两段键区间不重叠。
fn symbol_value_key(idx: u32) -> u32 {
    if idx < WELL_KNOWN_SYMBOL_COUNT {
        make_well_known_symbol_key(idx)
    } else {
        make_symbol_key(idx)
    }
}

impl Vm {
    /// 把 `JsValue` 转为属性键 si（字符串 intern id，`u32`）。
    ///
    /// 非负小整数（含整值 double）直接编码到整数键区间，免 to_string + intern；
    /// 字符串中形如数组下标的规范数字串（`"5"`）映射到同一整数键，保证
    /// `obj["5"]` 与 `obj[5]` 键等价。
    ///
    /// # 边界与前提
    /// - 索引超 `INT_KEY_COUNT`（2^30）时回退字符串键路径（intern + 反查仍正确）
    /// - 负数与小数不进入整数键区间（`arr[-1]`/`arr[1.5]` 是普通字符串键）
    pub(crate) fn property_key_si(&mut self, val: JsValue) -> Result<u32, String> {
        if val.is_int() {
            let i = val.as_int();
            if i >= 0 && (i as u32) < INT_KEY_COUNT {
                return Ok(make_int_key(i as u32));
            }
        } else if val.is_double() {
            let d = val.as_double();
            if d >= 0.0 && d.fract() == 0.0 && d < INT_KEY_COUNT as f64 {
                return Ok(make_int_key(d as u32));
            }
        } else if val.is_string() {
            // SAFETY: val 是字符串值，按载荷形态桥接为永久 key id。
            let s = unsafe { &*val.as_string_ptr() };
            if s.is_flat() {
                // Flat：良形 UTF-8 文本，走与单元路径同口径的编码入键空间。
                return Ok(self.string_key_text(s.as_str()));
            }
            // 单元载荷：键推导同规范（见 string_key_units）。
            return Ok(self.string_key_units(&s.units()));
        }
        // Symbol 值直接编码为 Symbol 键（不进字符串 interner，键相互独立）；
        // well-known 下标占保留槽，用户符号下标须加偏移，两段键区间不重叠。
        if val.is_symbol() {
            return Ok(symbol_value_key(val.as_symbol_index()));
        }
        // 遗留的 well-known symbol 空对象：按指针比对映射到各自的 well-known Symbol 键，
        // 避免全部塌缩成同一个键。
        if val.is_object() {
            if let Some(id) = oxide_runtime_api::well_known_symbol_id(self, val.as_js_object_ptr()) {
                return Ok(make_well_known_symbol_key(id));
            }
            // ToPropertyKey：对象经 ToPrimitive(string hint)，结果为 Symbol 时直接作键；
            // 其余字符串按单元序列推导键（避免 lossy 文本桥接破坏孤立 surrogate 键）。
            let prim = coercion::to_primitive(val, coercion::ToPrimitiveHint::String, self)?;
            if prim.is_symbol() {
                return Ok(symbol_value_key(prim.as_symbol_index()));
            }
            let units = oxide_runtime_api::to_units_full(prim, self)?;
            return Ok(self.string_key_units(&units));
        }
        // 其它原始值（BigInt 等）：ToPropertyKey 一律转字符串并走规范化，避免与
        // 数字键区间分裂（`o[5n]` 与 `o["5"]`/`o[5]` 必须同键）。
        let units = oxide_runtime_api::to_units_full(val, self)?;
        Ok(self.string_key_units(&units))
    }

    /// 从良形文本串推导属性键 si（`property_key_si` Flat 分支与 `string_key_si`
    /// 条目的共享入口）：规范数组下标 → 整数键，其余以 `encode_key` 形态入键空间
    /// ——无 FFFD 的良形文本编码恒等，含 FFFD 的键与单元路径（Cons 载荷、
    /// `string_key_si`）收敛到同一编码形态，同一逻辑键不因构造路径分裂。
    pub(crate) fn string_key_text(&self, text: &str) -> u32 {
        if let Some(i) = canonical_index_of(text) {
            return make_int_key(i);
        }
        if text.contains('\u{FFFD}') {
            let units: Vec<u16> = text.encode_utf16().collect();
            let key = oxide_kernel::string_forge::encode_key(&units);
            self.kernel_core.perm_interner().intern(&key).0
        } else {
            self.kernel_core.perm_interner().intern(text).0
        }
    }

    /// 从单元序列推导属性键 si（`property_key_si` 字符串分支的口径抽取）：
    /// 规范数组下标 → 整数键，其余以 `encode_key` 形态入键空间（孤立 surrogate
    /// 以转义形态区分，不与良形键碰撞）。
    pub(crate) fn string_key_units(&self, units: &[u16]) -> u32 {
        if let Some(i) = canonical_index_units(units) {
            make_int_key(i)
        } else {
            let key = oxide_kernel::string_forge::encode_key(units);
            self.kernel_core.perm_interner().intern(&key).0
        }
    }

    /// 属性键反解数组下标：整数键直取，字符串键须无前导零且 `u32` 解析成功。
    pub(crate) fn array_index_from_property_key(&self, prop_name_si: u32) -> Option<u32> {
        if is_int_key(prop_name_si) {
            return Some(int_key_value(prop_name_si));
        }
        let key = self.kernel_core.perm_interner().lookup(prop_name_si)?;
        if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
            return None;
        }
        key.parse::<u32>().ok()
    }

    /// 删除属性引用基非对象面（delete 成员表达式的 ToObject 基求值）：
    /// null/undefined 基 ToObject 抛 TypeError（规范 12.5.3.2 步 5.b）；其余原始
    /// 基装箱后按自身属性集判删除成败——字符串装箱体自身属性恰为规范下标
    /// 0..len-1 与 "length"（均不可配置，删除失败返回 false），其它键与其它
    /// 原始装箱体（number/boolean/BigInt/Symbol 无自身属性）删除成功返回
    /// true。
    ///
    /// # 步骤
    /// 1. null/undefined 基抛 TypeError 并早返回（异常已展开，不再写结果槽）
    /// 2. 字符串基按下标键区间判自身属性；非字符串基恒真
    /// 3. 严格模式下删除失败（字符串下标/length 不可配置）抛 TypeError
    /// 4. 布尔结果写 `rd` 槽（与对象基臂同槽位）
    ///
    /// # 边界与前提
    /// - 调用点已证基非对象；对象基走既有对象臂，不进本函数
    /// - 字符串长度口径按 UTF-16 码元（与字符串自身属性面一致）；非规范数字串
    ///   键（前导零等）与 symbol 键走"无自身属性"面
    ///
    /// # 副作用
    /// - 抛错路径经 `unwind` 改写 pc / 异常通道；成功路径写 `regs[rd]`
    pub(crate) fn delete_prop_non_object_base(
        &mut self, base: JsValue, rd: usize, key_si: u32,
    ) -> Result<bool, String> {
        if base.is_null() || base.is_undefined() {
            self.raise_type_error("delete on non-object")?;
            return Ok(true);
        }
        let deleted = if base.is_string() {
            let len = unsafe { (*base.as_string_ptr()).units().len() };
            match self.array_index_from_property_key(key_si) {
                Some(i) if (i as usize) < len => false,
                Some(_) => true,
                None => key_si != self.length_si,
            }
        } else {
            true
        };
        // 删除失败即命中不可配置自身属性，严格模式下 delete 运算符须抛 TypeError。
        if !deleted && self.current_strict() {
            self.raise_type_error("Cannot delete property")?;
            return Ok(true);
        }
        self.regs[rd] = JsValue::bool(deleted);
        Ok(false)
    }

    /// 属性读值解析：按数组 length 虚拟属性、元素区、shape 槽、原型链的顺序
    /// 查找，命中即返回，全链 miss 返回 `None`。
    ///
    /// 数组元素区的 hole（删除标记）视同不存在；原型链查找止于
    /// `MAX_PROTO_CHAIN_DEPTH` 深度上限。
    pub(crate) fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        vm_trace!("resolve_property: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let length_si = self.length_si;
        if obj.is_array() && prop_name_si == length_si {
            return Some(obj.logical_len_value());
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                // 数组元素区：hole（删除标记）视为不存在。
                if index < obj.array_prop_count && !obj.prop_meta_at(index).is_some_and(|m| m.is_hole()) {
                    return Some(obj.get_prop_at(index));
                }
            }
        }
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self
                .kernel_core
                .shape_forge()
                .lookup_position(proto_obj.shape_id(), prop_name_si)
            {
                let val = proto_obj.get_prop_at(pos);
                if !val.is_undefined() || proto_obj.prop_vec_len() > pos as usize {
                    return Some(val);
                }
            }
            proto = proto_obj.proto();
        }
        None
    }

    /// 存在性判定（规范 HasProperty）：自身与原型链逐级检查，任一层 P 为自有
    /// 属性即存在。数组 length 虚拟属性恒存在；数组元素区 hole 视同缺失；
    /// 其余按 shape 槽判定。
    ///
    /// 顶层 TypedArray 经统一数值键门（exotic [[HasProperty]]）：界内整数索引
    /// 判存在；数字无效键（负/分数/±Infinity/NaN/越界，含 "-0" 特例）立即
    /// false，不查自身命名属性也不走原型链；非规范数字串落普通路径。原型链上
    /// 的 TA 按普通对象查 shape 槽。
    ///
    /// 与 `resolve_property` 的差异：原型链上每层都检查数组元素区
    /// （`resolve_property` 只在顶层检查元素区，链上仅走 shape 槽），
    /// 继承自父数组的索引属性在此判存在。
    pub(crate) fn has_property(&self, obj: &JsObject, prop_name_si: u32) -> bool {
        let length_si = self.length_si;
        let mut current = Some(obj);
        let mut depth = 0usize;
        while let Some(obj) = current {
            if obj.is_array() && prop_name_si == length_si {
                return true;
            }
            // 统一数值键门（exotic [[HasProperty]]）：同 `ordinary_get_inner`
            // 的口径，门只落在顶层对象。
            if obj.is_typed_array_obj() && depth == 0 {
                match oxide_builtins::typed_array::ta_index_gate(self, obj, prop_name_si) {
                    oxide_builtins::typed_array::TaIndexGate::NumericValid(_) => return true,
                    oxide_builtins::typed_array::TaIndexGate::NumericInvalid => return false,
                    oxide_builtins::typed_array::TaIndexGate::Ordinary => {}
                }
            }
            if self.get_own_property_slot(obj, prop_name_si).is_some() {
                return true;
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                break;
            }
            depth += 1;
            let proto = obj.proto();
            current = proto.is_object().then(|| unsafe { &*proto.as_js_object_ptr() });
        }
        false
    }

    /// 查找自身属性槽下标：数组 length 虚拟属性返回 `None`，元素区返回下标
    /// （hole 视缺失），shape 槽命中即属性在场并返回存储下标（数组加元素区
    /// 偏移，普通对象槽位即下标）；值可为显式 undefined，不参与存在性判定；
    /// 未命中返回 `None`。
    pub(crate) fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        let length_si = self.length_si;
        if obj.is_array() && prop_name_si == length_si {
            return None;
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                // 数组元素区：hole（删除标记）视为不存在。
                if index < obj.array_prop_count && !obj.prop_meta_at(index).is_some_and(|m| m.is_hole()) {
                    return Some(index);
                }
            }
        }
        self.kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
            .and_then(|pos| {
                if obj.is_array() {
                    // 数组属性存储索引 = array_prop_count + shape 槽位（与元素区分）。
                    // shape 槽命中即属性在场：删除重建保证槽随属性移除，
                    // 显式 undefined 值同样在场（规范存在性只看自身槽）。
                    Some(obj.array_prop_count + pos)
                } else {
                    let val = obj.get_prop_at(pos);
                    if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                        Some(pos)
                    } else {
                        None
                    }
                }
            })
    }
}
