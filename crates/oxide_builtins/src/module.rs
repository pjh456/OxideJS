//! 模块命名空间与求值辅助（import 实现内部用，非 JS 可见标准 API）。
//!
//! 命名约定 `__module*`：由编译器模块 prelude 发出（`lookup_or_builtin` 解析为全局槽，
//! VM 在 frame push 时从 global 对象取回 native 函数）。当前为快照式链接：
//! 依赖模块先整体求值并返回命名空间对象，导入方从中读取导出值；
//! live binding / source-phase / defer 语义留待后续轮次。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::{Cell, JsObject, PropAttributes, PropMetaEntry};
use oxide_types::private_key::make_well_known_symbol_key;
use oxide_types::value::JsValue;

/// `Symbol.toStringTag` 在 well-known symbol 表中的序号，命名空间标签键按其编码。
const TO_STRING_TAG_SYMBOL_ID: u32 = 9;

/// 未初始化导出被读时的 ReferenceError 文本。
///
/// exotic `[[Get]]` 与 `[[GetOwnProperty]]` 消费端（`Object.*`、for-in）共用同一
/// 文本，错误类型由消费端的 `create_reference_error` / `create_from_text` 恢复。
pub const NS_UNINITIALIZED_MESSAGE: &str = "Cannot access module export before initialization";

/// 模块命名空间导出属性描述符：可写、可枚举、不可配置（module namespace exotic）。
///
/// 规范 10.4.6.5 返回 `{writable: true, enumerable: true, configurable: false}`；
/// 写保护由命名空间的 `[[Set]]`/`[[DefineOwnProperty]]` 专属语义承担，不靠描述符位。
fn ns_attrs() -> PropAttributes {
    PropAttributes::new(true, true, false)
}

fn type_error<H: VmHost>(vm: &mut H, msg: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, msg))
}

/// 命名空间条目表的来源身份（provenance）。
///
/// `Local` 为本模块直接声明的导出；`Reexport` 描述跨模块再导出的 `(模块, 绑定)`
/// 身份，`direct` 区分显式再导出（`export { x } from`）与 star 复制的间接导出；
/// `Ambiguous` 是 star 冲突后的省略哨兵，该名不再出现在命名空间且一旦歧义恒歧义。
#[derive(Clone, Copy)]
pub enum ModuleNsOrigin {
    Local,
    Reexport { path_id: u32, binding_id: u32, direct: bool },
    Ambiguous,
}

/// 单个导出名的活值状态。
pub enum ModuleNsState {
    /// 预注册后未初始化的一般导出；`initialized` 为 true 时 `value` 是当前活值。
    Value { value: JsValue, initialized: bool },
    /// 被捕获导出挂的共享 cell，活值由 cell 承载。
    Cell(*mut Cell),
}

/// 命名空间条目：导出名 interned 键、活值状态与来源身份一一对应。
pub struct ModuleNsEntry {
    pub name_si: u32,
    pub origin: ModuleNsOrigin,
    pub state: ModuleNsState,
}

/// 模块命名空间条目表：挂在 ns 对象 `native_data` 上，是 exotic [[Get]] 的权威
/// 状态。键为 `PermInterner` interned 键，线性查找（导出名通常少于 50）。
pub struct ModuleNsTable {
    entries: Vec<ModuleNsEntry>,
}

/// 导出名状态查询结果。
pub enum ModuleNsQuery {
    /// 未初始化（预注册未赋值，或未初始化的共享 cell）。
    Uninitialized,
    /// 已初始化活值（`Value` 条目或已初始化 cell）。
    Initialized(JsValue),
}

/// 取 ns 对象的条目表指针；非 module namespace 或无表返回空指针。
///
/// # 注意事项
/// - 返回指针由 ns 对象 `native_data` 持有，在该对象存活期内有效；消费方须在
///   同一 VM 世代内使用。
fn ns_table_ptr(obj: &JsObject) -> *mut ModuleNsTable {
    if !obj.is_module_namespace() {
        return std::ptr::null_mut();
    }
    obj.native_data() as *mut ModuleNsTable
}

/// 在条目表中按 interned 键定位条目下标。
fn entry_index(table: &ModuleNsTable, key_si: u32) -> Option<usize> {
    table.entries.iter().position(|e| e.name_si == key_si)
}

/// 分配空条目表并挂到 ns 对象的 `native_data`。
fn install_ns_table(obj: &mut JsObject) -> *mut ModuleNsTable {
    let table = Box::into_raw(Box::new(ModuleNsTable { entries: Vec::new() }));
    obj.set_native_data(table as *mut u8);
    table
}

/// 追加一条未初始化的 `Value` 条目（来源为本地导出）。
///
/// # 注意事项
/// - 调用方保证 `name_si` 尚未入表（入口均为先查后插）。
fn push_uninitialized_entry(table: *mut ModuleNsTable, name_si: u32) {
    push_entry(
        table,
        name_si,
        ModuleNsOrigin::Local,
        ModuleNsState::Value {
            value: JsValue::undefined(),
            initialized: false,
        },
    );
}

/// 追加一条已初始化为 undefined 的 `Value` 条目（来源为本地导出）。
///
/// # 边界与前提
/// - 供 var 作用域导出（`VarScopedDeclarations`）在实例化期预初始化：读时为
///   undefined 而非 TDZ，写点由 `__moduleSet` 就地覆盖。
fn push_var_initialized_entry(table: *mut ModuleNsTable, name_si: u32) {
    push_entry(
        table,
        name_si,
        ModuleNsOrigin::Local,
        ModuleNsState::Value {
            value: JsValue::undefined(),
            initialized: true,
        },
    );
}

/// 向条目表追加一条带来源身份的条目。
///
/// # 注意事项
/// - table 归调用方持有的 ns 对象所有，在同一 VM 世代内有效。
fn push_entry(table: *mut ModuleNsTable, name_si: u32, origin: ModuleNsOrigin, state: ModuleNsState) {
    let entry = ModuleNsEntry { name_si, origin, state };
    // SAFETY: table 归调用方持有的 ns 对象所有，在同一 VM 世代内有效。
    unsafe {
        (*table).entries.push(entry);
    }
}

/// 模块命名空间字符串导出名的状态查询（不抛错、不分配、不写状态）。
///
/// # 边界与前提
/// - 返回 `None`：非 module namespace / 无条目表 / 键非条目（含 symbol 键）。
/// - 返回 `Uninitialized`：预注册未赋值，或条目挂在未初始化的共享 cell 上。
/// - 返回 `Initialized(v)`：`Value` 已初始化，或 cell 已初始化。
///
/// # 注意事项
/// - `Cell` 读依赖「cell 由 `alloc_cell` 分配并登记，至 `full_reset` 才统一释放」
///   的生命周期契约；同一 VM 世代内指针有效。
pub fn module_ns_export(obj: &JsObject, key_si: u32) -> Option<ModuleNsQuery> {
    let table = ns_table_ptr(obj);
    if table.is_null() {
        return None;
    }
    // SAFETY: table 指向本 ns 对象持有的 Box<ModuleNsTable>，随对象存活。
    let table_ref = unsafe { &*table };
    let entry = table_ref.entries.get(entry_index(table_ref, key_si)?)?;
    // 歧义哨兵条目：该名已从命名空间省略，查询一律视为非导出（落普通路径）。
    if matches!(entry.origin, ModuleNsOrigin::Ambiguous) {
        return None;
    }
    match &entry.state {
        ModuleNsState::Value { value, initialized } => {
            if *initialized {
                Some(ModuleNsQuery::Initialized(*value))
            } else {
                Some(ModuleNsQuery::Uninitialized)
            }
        }
        ModuleNsState::Cell(cell) => {
            if cell.is_null() {
                return Some(ModuleNsQuery::Uninitialized);
            }
            // SAFETY: cell 由 alloc_cell 分配并登记，至 full_reset 才释放。
            let cell_ref = unsafe { &**cell };
            if cell_ref.is_initialized() {
                Some(ModuleNsQuery::Initialized(cell_ref.value))
            } else {
                Some(ModuleNsQuery::Uninitialized)
            }
        }
    }
}

/// 收集条目表持有的 JsValue 边（已初始化 `Value` 与全部 `Cell` 内值），供 GC mark。
pub fn module_ns_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let table = ns_table_ptr(obj);
    if table.is_null() {
        return Vec::new();
    }
    // SAFETY: table 归本 ns 对象持有，生命周期见 `module_ns_export`。
    let table_ref = unsafe { &*table };
    let mut edges = Vec::new();
    for entry in &table_ref.entries {
        match &entry.state {
            ModuleNsState::Value { value, initialized } => {
                if *initialized {
                    edges.push(*value);
                }
            }
            ModuleNsState::Cell(cell) => {
                if !cell.is_null() {
                    // SAFETY: cell 生命周期见 `module_ns_export`。
                    edges.push(unsafe { (**cell).value });
                }
            }
        }
    }
    edges
}

/// 原地重写条目表内的 JsValue 边（对象搬移/epoch 晋升后的引用重定位）。
///
/// # 副作用
/// - 改写 `Value` 条目的值与共享 cell 内的值；cell 指针本身稳定不变。
pub fn rewrite_module_ns_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    let table = ns_table_ptr(obj);
    if table.is_null() {
        return;
    }
    // SAFETY: table 归本 ns 对象持有，原地改写其 JsValue 边。
    let table_ref = unsafe { &mut *table };
    for entry in table_ref.entries.iter_mut() {
        match &mut entry.state {
            ModuleNsState::Value { value, .. } => *value = rewrite(*value),
            ModuleNsState::Cell(cell) => {
                if !cell.is_null() {
                    // SAFETY: cell 生命周期见 `module_ns_export`；共享 cell 内值一并改写。
                    let cell_ref = unsafe { &mut **cell };
                    cell_ref.value = rewrite(cell_ref.value);
                }
            }
        }
    }
}

/// 深拷贝条目表到 `dst`，并用 `rewrite` 改写其中的对象引用。
///
/// # 注意事项
/// - `clone_for_session_epoch` 只浅拷贝 `native_data` 指针；本函数必须在新对象上
///   重建独立 Box，否则原件与克隆共享同一表会在释放时双放。
/// - `Cell` 指针本身跨搬移稳定，只改写其内部值。
pub fn clone_module_ns_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_module_namespace() {
        return;
    }
    let table = src.native_data() as *const ModuleNsTable;
    if table.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    // SAFETY: src.native_data 指向 src 持有的 Box<ModuleNsTable>。
    let table_ref = unsafe { &*table };
    let mut entries = Vec::with_capacity(table_ref.entries.len());
    for entry in &table_ref.entries {
        let state = match &entry.state {
            ModuleNsState::Value { value, initialized } => ModuleNsState::Value {
                value: if value.is_object() { rewrite(*value) } else { *value },
                initialized: *initialized,
            },
            ModuleNsState::Cell(cell) => {
                if !cell.is_null() {
                    // SAFETY: cell 生命周期见 `module_ns_export`；晋升/搬移后改写其内值。
                    let cell_ref = unsafe { &mut **cell };
                    cell_ref.value = rewrite(cell_ref.value);
                }
                ModuleNsState::Cell(*cell)
            }
        };
        entries.push(ModuleNsEntry {
            name_si: entry.name_si,
            origin: entry.origin,
            state,
        });
    }
    dst.set_native_data(Box::into_raw(Box::new(ModuleNsTable { entries })) as *mut u8);
}

/// 只读核算条目表的 native 数据字节（不释放），与 `drop_module_ns_native` 的
/// capacity 口径一致，供 GC 账目核算。
pub fn module_ns_native_size(obj: &JsObject) -> u64 {
    let table = ns_table_ptr(obj);
    if table.is_null() {
        return 0;
    }
    // SAFETY: table 归本 ns 对象持有，生命周期见 `module_ns_export`。
    let table_ref = unsafe { &*table };
    (std::mem::size_of::<ModuleNsTable>() + table_ref.entries.capacity() * std::mem::size_of::<ModuleNsEntry>()) as u64
}

/// 释放条目表（`Box<ModuleNsTable>`），返回释放字节数供泄漏统计。
///
/// # 注意事项
/// - 每对象的表至多释放一次；释放后 `native_data` 置空保证幂等。
pub fn drop_module_ns_native(obj: &mut JsObject) -> u64 {
    let table = ns_table_ptr(obj);
    if table.is_null() {
        return 0;
    }
    let bytes = module_ns_native_size(obj);
    // SAFETY: table 由 install_ns_table 的 Box::into_raw 分配，本处恰好释放一次。
    unsafe {
        drop(Box::from_raw(table));
    }
    obj.set_native_data(std::ptr::null_mut());
    bytes
}

/// `__modulePreRegister(ns, name, var_like)`：live 模块在 body 求值前预注册静态导出名。
///
/// # 步骤
/// 1. 校验 ns 为 module namespace 对象，取导出名的 interned 键。
/// 2. 无表时分配；键已有条目时 no-op（幂等）。
/// 3. 经瞬态可扩展窗口在 ns 上定义真实数据属性 `undefined`（可写可枚举不可配置）。
/// 4. 追加条目：`var_like` 为真（var/函数类导出，属 `VarScopedDeclarations`）时
///    初始化为 undefined；否则保持未初始化（lexical/class 的 TDZ）。随后按规范顺序
///    重排导出键。
///
/// # 副作用
/// - 在 ns 上定义真实属性槽、挂载/扩展条目表，并重排 `[[OwnPropertyKeys]]` 顺序。
pub fn module_pre_register<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return type_error(vm, "__modulePreRegister: 3 arguments required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__modulePreRegister: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__modulePreRegister: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let name_val = vm.reg(args[2]);
    let name_si = vm.property_key_si(name_val);
    let var_like = oxide_runtime_api::to_boolean(vm.reg(args[3]));
    let obj = unsafe { &mut *ns_ptr };
    if !obj.is_module_namespace() {
        return type_error(vm, "__modulePreRegister: target is not a module namespace");
    }

    // 幂等：同名条目已存在时不再定义真实槽或重复入表。
    let existing = ns_table_ptr(obj);
    if !existing.is_null() && unsafe { entry_index(&*existing, name_si) }.is_some() {
        return NativeResult::Ok(JsValue::undefined());
    }

    // 导出定义窗口：同 module_set，窗口内不运行 JS，错误路径先复位扩展性。
    obj.set_extensible(true);
    let result = vm.define_data_property(obj, name_si, JsValue::undefined(), ns_attrs());
    obj.set_extensible(false);
    if let Err(e) = result {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }

    let table = if existing.is_null() { install_ns_table(obj) } else { existing };
    if var_like {
        push_var_initialized_entry(table, name_si);
    } else {
        push_uninitialized_entry(table, name_si);
    }
    crate::object::sort_namespace_exports(vm, obj);
    NativeResult::Ok(JsValue::undefined())
}

/// `__moduleObject()`：创建模块命名空间对象（null 原型，带 @@toStringTag Symbol 键）。
///
/// # 副作用
/// - 对象创建起即标记 module namespace exotic 并置 non-extensible：导出定义由
///   `module_set`/`module_star` 的瞬态可扩展窗口旁路，其余写/定义一律拒绝。
pub fn module_object<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let tag_si = make_well_known_symbol_key(TO_STRING_TAG_SYMBOL_ID);
    let tag_val = vm.new_string("Module");
    let obj_ref = unsafe { &mut *obj };
    if let Err(e) = vm.define_data_property(obj_ref, tag_si, tag_val, PropAttributes::new(false, false, false)) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    obj_ref.set_module_namespace(true);
    obj_ref.set_extensible(false);
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// 写入命名空间导出：命中条目则更新活值、真实属性槽与来源身份；否则经瞬态可扩展
/// 窗口定义真实属性槽，并在 ns 有条目表时追加条目。
///
/// # 边界与前提
/// - `Cell` 条目 no-op：被捕获导出的活值由共享 cell 承载，直接写槽会与 cell 分裂。
/// - 条目存在但真实槽缺失（歧义省略后显式导出重新定义）：落新键定义路径补回槽。
///
/// # 副作用
/// - 修改条目表状态/来源身份与 ns 属性槽；新键或删槽重定义时重排
///   `[[OwnPropertyKeys]]` 顺序。
fn set_export<H: VmHost>(
    vm: &mut H, obj: &mut JsObject, name_si: u32, value: JsValue, origin: ModuleNsOrigin,
) -> Result<(), String> {
    let table = ns_table_ptr(obj);
    let existing = if table.is_null() {
        None
    } else {
        // SAFETY: table 归本 ns 对象持有，生命周期见 `module_ns_export`。
        unsafe { entry_index(&*table, name_si) }
    };

    // Cell 条目 no-op：被捕获导出的活值由共享 cell 承载，写槽会与 cell 分裂。
    if let Some(idx) = existing {
        // SAFETY: idx 来自刚完成的表内查找，仍在界内。
        if matches!(unsafe { &(&(*table).entries)[idx].state }, ModuleNsState::Cell(_)) {
            return Ok(());
        }
    }

    // 条目已存在且真实槽仍在：原地更新活值/来源，键集不变无需重排。
    if let Some(idx) = existing {
        if let Some(pos) = vm.get_own_property_slot(obj, name_si) {
            obj.set_prop_at(pos, value);
            // SAFETY: idx 来自刚完成的表内查找，仍在界内。
            unsafe {
                (&mut (*table).entries)[idx].origin = origin;
                (&mut (*table).entries)[idx].state = ModuleNsState::Value { value, initialized: true };
            }
            return Ok(());
        }
        // 歧义省略删槽后显式导出重新定义：落下方定义路径补回真实槽。
    }

    // 导出定义窗口：命名空间创建起不可扩展，定义新导出须瞬态放开扩展性。窗口内
    // 不运行 JS，无可观察副作用；错误路径先复位以维持 non-extensible 不变式。
    obj.set_extensible(true);
    let result = vm.define_data_property(obj, name_si, value, ns_attrs());
    obj.set_extensible(false);
    result?;

    if !table.is_null() {
        if let Some(idx) = existing {
            // SAFETY: idx 来自刚完成的表内查找；重定义后同步条目，勿重复入表。
            unsafe {
                (&mut (*table).entries)[idx].origin = origin;
                (&mut (*table).entries)[idx].state = ModuleNsState::Value { value, initialized: true };
            }
        } else {
            push_entry(table, name_si, origin, ModuleNsState::Value { value, initialized: true });
        }
    }
    // 每次键集变化后重排：自导入在 body 内即可见有序的 [[OwnPropertyKeys]]。
    crate::object::sort_namespace_exports(vm, obj);
    Ok(())
}

/// 从命名空间属性表移除一个导出槽（歧义省略用），保留其余字符串与 Symbol 键及其
/// 描述符。
///
/// # 注意事项
/// - 命名空间导出属性为 non-configurable，本函数不经 `[[Delete]]`，直接重建属性表
///   以同时保住 `@@toStringTag` Symbol 键。
fn remove_export_slot<H: VmHost>(vm: &mut H, obj: &mut JsObject, name_si: u32) {
    let mut retained: Vec<(u32, JsValue, Option<PropMetaEntry>)> = crate::object::walk_own_keys(vm, obj)
        .into_iter()
        .filter(|(si, _)| *si != name_si)
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();
    let symbols: Vec<(u32, JsValue, Option<PropMetaEntry>)> = crate::object::walk_own_symbol_keys(vm, obj)
        .into_iter()
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();
    obj.set_shape_id(EMPTY_SHAPE_ID);
    obj.clear_props();
    for (si, value, meta) in retained.drain(..).chain(symbols) {
        let shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape);
        let pos = obj.push_prop(value);
        if let Some(meta) = meta {
            if meta.is_accessor {
                obj.set_accessor_meta(pos, meta.get, meta.set, meta.attributes);
            } else {
                obj.set_data_meta(pos, meta.attributes);
            }
        }
    }
    obj.bump_generation();
}

/// `__moduleSet(ns, name, value)`：在命名空间上定义或更新本地导出属性。
///
/// # 副作用
/// - 同 `set_export`；本地导出来源身份恒为 `Local`，覆盖先前 star 条目时复位来源。
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
    if let Err(e) = set_export(vm, obj, name_si, value, ModuleNsOrigin::Local) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `__moduleSetReexport(ns, name, value, dep_path, imported)`：注册显式再导出
/// （`export { x } from` / `export * as ns from` / 导入绑定再导出），记录跨模块
/// 来源身份 `(dep_path, imported)`，使同一绑定经不同路径转发时来源 token 相等。
///
/// # 副作用
/// - 同 `module_set`；ns 无条目表时惰性安装，来源标为 `direct` 再导出。
pub fn module_set_reexport<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 6 {
        return type_error(vm, "__moduleSetReexport: 5 arguments required");
    }
    let ns_val = vm.reg(args[1]);
    let ns_ptr = match vm.checked_object_ptr(ns_val, "__moduleSetReexport: target is not an object") {
        Ok(Some(p)) => p,
        Ok(None) => return type_error(vm, "__moduleSetReexport: target is not an object"),
        Err(e) => return NativeResult::Err(crate::error::create_error(vm, &e)),
    };
    let name_val = vm.reg(args[2]);
    let value = vm.reg(args[3]);
    let path_val = vm.reg(args[4]);
    let imported_val = vm.reg(args[5]);
    let name_si = vm.property_key_si(name_val);
    let Some(dep_path) = vm.lookup_str(path_val) else {
        return type_error(vm, "__moduleSetReexport: module path is not a string");
    };
    let Some(imported) = vm.lookup_str(imported_val) else {
        return type_error(vm, "__moduleSetReexport: imported name is not a string");
    };
    let obj = unsafe { &mut *ns_ptr };
    if !obj.is_module_namespace() {
        return type_error(vm, "__moduleSetReexport: target is not a module namespace");
    }
    if ns_table_ptr(obj).is_null() {
        install_ns_table(obj);
    }
    let path_id = vm.kernel_core().perm_interner().intern(&dep_path).0;
    let binding_id = vm.kernel_core().perm_interner().intern(&imported).0;
    let origin = ModuleNsOrigin::Reexport {
        path_id,
        binding_id,
        direct: true,
    };
    if let Err(e) = set_export(vm, obj, name_si, value, origin) {
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
        return NativeResult::Err(crate::error::create_syntax_error(vm, "requested module export is not exported"));
    }
    match vm.ordinary_get(obj, name_si, ns_val) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(crate::error::create_error(vm, &e)),
    }
}

/// 取 src 命名空间某导出名的来源身份。
///
/// # 边界与前提
/// - src 条目表命中且来源为 `Reexport`：沿用其 `(模块, 绑定)`，`direct` 归一为
///   false（经 star 复制到 dst 的是间接提供）。
/// - 其余（无表 / `Local` / 条目缺失）：回退 `(src_path_id, name_si)`。只声明本地
///   导出的源不建条目表，回退 token 使不同源的同名本地导出身份不同。
fn star_source_origin(src: &JsObject, name_si: u32, src_path_id: u32) -> ModuleNsOrigin {
    let table = ns_table_ptr(src);
    if !table.is_null() {
        // SAFETY: table 归 src 持有，生命周期见 `module_ns_export`。
        if let Some(idx) = unsafe { entry_index(&*table, name_si) } {
            // SAFETY: idx 来自刚完成的表内查找，仍在界内。
            if let ModuleNsOrigin::Reexport { path_id, binding_id, .. } = unsafe { (&(*table).entries)[idx].origin } {
                return ModuleNsOrigin::Reexport {
                    path_id,
                    binding_id,
                    direct: false,
                };
            }
        }
    }
    ModuleNsOrigin::Reexport {
        path_id: src_path_id,
        binding_id: name_si,
        direct: false,
    }
}

/// `__moduleStar(dst, src, src_path)`：把 src 命名空间的可枚举导出（除 default）
/// 复制到 dst，并按来源身份解析同名冲突。
///
/// # 步骤
/// 1. 每个 src 导出名取来源 `(模块, 绑定)`；`default` 跳过。
/// 2. dst 条目为 `Ambiguous`：恒跳过（一旦歧义恒省略）。
/// 3. dst 条目为显式导出（`Local` / direct `Reexport`）：显式恒胜，跳过。
/// 4. dst 条目为间接 `Reexport`：`(模块, 绑定)` 相同则保留；不同则删槽并记
///    `Ambiguous`。
/// 5. dst 无条目：真实槽已存在视为显式导出跳过；否则复制槽并记间接来源。
///
/// # 副作用
/// - 在 dst 上定义/删除真实属性槽，惰性安装条目表，并按规范顺序重排导出键。
pub fn module_star<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return type_error(vm, "__moduleStar: 3 arguments required");
    }
    let dst_val = vm.reg(args[1]);
    let src_val = vm.reg(args[2]);
    let src_path_val = vm.reg(args[3]);
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
    let Some(src_path) = vm.lookup_str(src_path_val) else {
        return type_error(vm, "__moduleStar: source path is not a string");
    };
    let src_path_id = vm.kernel_core().perm_interner().intern(&src_path).0;
    let src = unsafe { &*src_ptr };
    let default_si = vm.kernel_core().perm_interner().intern("default").0;
    let keys = crate::object::walk_own_keys(vm, src);
    let dst = unsafe { &mut *dst_ptr };
    for (name_si, pos) in keys {
        if name_si == default_si {
            continue;
        }
        let src_origin = star_source_origin(src, name_si, src_path_id);
        // live dst：star 复制的新导出名须一并入表，否则活值读/枚举看不到条目。
        let dst_table = ns_table_ptr(dst);
        let dst_idx = if dst_table.is_null() {
            None
        } else {
            unsafe { entry_index(&*dst_table, name_si) }
        };
        match dst_idx {
            Some(idx) => {
                // SAFETY: idx 来自刚完成的表内查找，仍在界内。
                let dst_origin = unsafe { (&(*dst_table).entries)[idx].origin };
                match dst_origin {
                    ModuleNsOrigin::Ambiguous => continue,
                    ModuleNsOrigin::Local | ModuleNsOrigin::Reexport { direct: true, .. } => continue,
                    ModuleNsOrigin::Reexport { path_id, binding_id, .. } => {
                        let ModuleNsOrigin::Reexport {
                            path_id: sp, binding_id: sb, ..
                        } = src_origin
                        else {
                            continue;
                        };
                        if path_id == sp && binding_id == sb {
                            continue;
                        }
                        // 不同绑定同名：删除导出槽并记歧义哨兵，清空可回收的活值边。
                        remove_export_slot(vm, dst, name_si);
                        // SAFETY: idx 仍指向本表条目，删除槽不改动条目表。
                        unsafe {
                            (&mut (*dst_table).entries)[idx].origin = ModuleNsOrigin::Ambiguous;
                            (&mut (*dst_table).entries)[idx].state = ModuleNsState::Value {
                                value: JsValue::undefined(),
                                initialized: false,
                            };
                        }
                    }
                }
            }
            None => {
                // 真实槽已存在但无条目：视为先前显式导出（本地导出不建表），显式恒胜。
                if vm.get_own_property_slot(dst, name_si).is_some() {
                    continue;
                }
                let value = src.get_prop_at(pos);
                let table = if dst_table.is_null() { install_ns_table(dst) } else { dst_table };
                // star 导出同 module_set：定义新导出走瞬态可扩展窗口，错误路径先复位。
                dst.set_extensible(true);
                let result = vm.define_data_property(dst, name_si, value, ns_attrs());
                dst.set_extensible(false);
                if let Err(e) = result {
                    return NativeResult::Err(crate::error::create_error(vm, &e));
                }
                // 快照语义：star 名立即视为已初始化。
                push_entry(table, name_si, src_origin, ModuleNsState::Value { value, initialized: true });
            }
        }
    }
    // star 复制后重排：直接导出与 star 导出的合并键序保持规范顺序。
    crate::object::sort_namespace_exports(vm, dst);
    NativeResult::Ok(JsValue::undefined())
}

/// `__moduleSeal(ns)`：封冻命名空间（不可扩展）。
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
///
/// # 副作用
/// - 同 `__moduleObject`：标记 module namespace exotic 并置 non-extensible。
pub fn module_data<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return type_error(vm, "__moduleData: 2 arguments required");
    }
    let kind_val = vm.reg(args[1]);
    let content_val = vm.reg(args[2]);
    if !kind_val.is_string() || !content_val.is_string() {
        return type_error(vm, "__moduleData: kind/content must be strings");
    }
    let kind = oxide_runtime_api::to_string(kind_val);
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
            // lossy 文本拷贝出 owned String 再入堆（模块数据文本桥接，131.2 边界）。
            let text = vm.lookup_str(content_val).unwrap_or_default();
            vm.new_string_owned(text)
        }
        _ => return type_error(vm, "__moduleData: unsupported data kind"),
    };
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let default_si = vm.kernel_core().perm_interner().intern("default").0;
    let obj_ref = unsafe { &mut *obj };
    if let Err(e) = vm.define_data_property(obj_ref, default_si, default_val, ns_attrs()) {
        return NativeResult::Err(crate::error::create_error(vm, &e));
    }
    obj_ref.set_module_namespace(true);
    obj_ref.set_extensible(false);
    NativeResult::Ok(JsValue::from_js_object(obj))
}
