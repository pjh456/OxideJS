//! ShadowRealm native 实现：构造器（建影子 realm 与实例对象）与 `evaluate` /
//! `importValue` 真实现（影子 realm 脚本执行、跨 realm 结果结算、WrappedFunction
//! 包装、模块文件加载）。
//!
//! 关键约定：
//! - 函数是 concrete `&mut Vm` 形态（同 `worker_constructor`），不挂 `VmHost`
//!   trait：构造器要新建 Vm（影子 realm），`VmHost` 接口面表达不了。
//! - 实例对象是 PLAIN 对象（`[[Prototype]]` → ShadowRealm.prototype），主 Vm
//!   侧表 `shadow_realm_objects` 是 brand 校验依据（this 指针在侧表），不占
//!   OBJ_TYPE 槽位。
//! - `evaluate` / `importValue` 真实现：在影子 realm 内编译执行（脚本 / 模块），
//!   结果经跨 realm 结算（原始值直返、函数包装为 WrappedFunction、非原始非函数
//!   抛 TypeError、符号按 realm 重映射）后返回。
//! - WrappedFunction 是调用方 realm 的函数对象（native 函数项），持有跨 realm
//!   逻辑边：原函数在属主 Vm 的保活表（`shadow_keepalive`，属主 Vm 的 GC 根）
//!   保活，包装方 Vm 经 `wrapped_function_state`（包装方 Vm 的 GC 根）反查。
//!   包装方向双向：evaluate 结果包装影子函数（包装在主 realm），包装函数实参
//!   包装主 realm 函数（包装在影子 realm），嵌套影子 realm 沿属主链上溯解析。

use std::sync::Arc;

use oxide_builtins::error;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_integer_or_infinity, NativeResult, ProtoKind, VmHost};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::WELL_KNOWN_SYMBOL_COUNT;
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;

/// `new ShadowRealm()`：建影子 realm（独立 Vm）与实例对象。
///
/// # 步骤
/// 1. 构造形态校验（规范无 `[[Call]]`：非构造调用抛 TypeError）。
/// 2. 建影子 Vm：复用主 Vm 的 `KernelCore`（共享 intern/shape/code 缓存）并
///    注入同一动态编译服务。
/// 3. 建实例对象：PLAIN 对象，`[[Prototype]]` → ShadowRealm.prototype。
/// 4. 登记主 Vm 侧表：键 = 影子 realm 编号，值 = （实例对象，影子 Vm）。
/// 5. 返回实例对象。
///
/// # 边界与前提
/// - 影子 Vm 存主 Vm 侧表、不跨线程：`full_reset` 与 `Drop` 统一清理。
/// - 实例对象是 GC 根（侧表登记）；影子 Vm 堆独立，主 Vm 的 GC 不触达。
///
/// # 副作用
/// - 新建一个影子 Vm（新 session 加全量 builtin world）；登记一条侧表条目
///   （GC 根）。
pub(crate) fn shadow_realm_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    // 构造形态校验（规范只有 [[Construct]]）。
    if !vm.constructing_native {
        return NativeResult::Err(error::create_type_error(vm, "ShadowRealm constructor requires new"));
    }

    // 建影子 Vm：共享主 Vm 的 KernelCore 与动态编译服务。
    let mut shadow_vm = Vm::with_kernel_core(vm.kernel_core.clone());
    shadow_vm.set_compiler_service(vm.compiler.clone());
    // 属主链：影子 Vm 记录创建它的 Vm，跨 realm 结算沿链上溯解析远端 realm。
    shadow_vm.owner_vm = vm as *mut Vm;

    // 建实例对象：PLAIN 对象，[[Prototype]] → ShadowRealm.prototype。
    let proto_val = shadow_realm_prototype_value(vm);
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
    let ptr = vm.alloc_object(obj);

    // 登记主 Vm 侧表（键 = 影子 realm 编号，值 = 实例对象加影子 Vm）。
    let realm_id = shadow_vm.realm_id() as u64;
    vm.shadow_realm_objects
        .insert(realm_id, (JsValue::from_js_object(ptr), shadow_vm));

    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// `ShadowRealm.prototype.evaluate`：在影子 realm 内把 `source` 按普通脚本编译
/// 执行，结果经跨 realm 结算后返回。
///
/// # 步骤
/// 1. brand 校验（`this` 须是 ShadowRealm 实例）。
/// 2. `source` 非字符串抛 TypeError（规范只接受 String，不做 ToString）。
/// 3. 在影子 realm 内把 `source` 编译为普通脚本（编译期 SyntaxError 原样透传）。
/// 4. 执行脚本（运行期异常包 TypeError）。
/// 5. 结果结算：原始值直返、函数包装为 WrappedFunction、符号按 realm 重映射、
///    非原始非函数抛 TypeError。
///
/// # 边界
/// - 编译期 SyntaxError 透传（规范把编译期异常暴露给调用方 realm）。
/// - 运行期异常（含影子 realm 内的 TypeError/ReferenceError 与原始值抛出）
///   在调用方 realm 包装为 TypeError。
pub(crate) fn evaluate(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if let Err(e) = shadow_realm_brand_check(vm, this_val) {
        return NativeResult::Err(e);
    }
    let realm_id = shadow_realm_id_of(vm, this_val.as_js_object_ptr());

    // 实参 source（args[1]）：非字符串抛 TypeError。
    let source_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let source = match vm.lookup_str(source_val) {
        Some(s) => s,
        None => {
            return NativeResult::Err(error::create_type_error(
                vm,
                "ShadowRealm.prototype.evaluate source must be a string",
            ))
        }
    };

    // 在影子 realm 内编译并执行：编译期 SyntaxError 原样透传，运行期异常包 TypeError。
    let raw_result: Result<JsValue, String> = {
        let shadow_vm = &mut vm.shadow_realm_objects.get_mut(&realm_id).unwrap().1;
        let script = match shadow_vm.create_plain_dynamic_script(&source) {
            Ok(s) => s,
            Err(msg) => return NativeResult::Err(error::create_syntax_error(vm, &msg)),
        };
        let global = shadow_vm.global_object();
        let global_val = JsValue::from_js_object(global.as_ptr() as *mut JsObject);
        shadow_vm.call_function_sync(script, global_val, &[])
    };
    let value = match raw_result {
        Ok(v) => v,
        Err(_) => return NativeResult::Err(error::create_type_error(vm, "Error evaluating code in ShadowRealm")),
    };

    // 结果结算：GetWrappedValue(callerRealm, value)。
    match settle_value(vm, vm.realm_id() as u64, realm_id, value) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// `ShadowRealm.prototype.importValue`：在影子 realm 内按 `specifier` 解析并加载
/// 模块文件，编译求值后取 `exportName` 的导出值，以 Promise 结算返回。
///
/// # 步骤
/// 1. brand 校验（`this` 须是 ShadowRealm 实例）。
/// 2. `ToString(specifier)`（abrupt completion 原值传播）；`exportName` 须为
///    字符串，非字符串抛 TypeError。
/// 3. 建 Promise 能力（调用方 realm）。
/// 4. 在影子 realm 内编译并求值模块（语法错误 / 求值抛错 / 导出缺失均 reject
///    TypeError）。
/// 5. 导出值结算（原始值直返、函数包装、符号重映射、对象 reject TypeError）。
/// 6. 返回 Promise 本体。
///
/// # 边界
/// - 模块解析失败（语法错误）与求值抛错以 TypeError reject。
/// - 导出值非原始非函数以 TypeError reject。
pub(crate) fn import_value(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if let Err(e) = shadow_realm_brand_check(vm, this_val) {
        return NativeResult::Err(e);
    }
    let realm_id = shadow_realm_id_of(vm, this_val.as_js_object_ptr());

    // ToString(specifier)：abrupt completion 传播原异常。
    let specifier_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let specifier = match oxide_runtime_api::to_string_full(specifier_val, vm) {
        Ok(s) => s,
        Err(e) => {
            let exc = vm.exception_value.take().unwrap_or_else(|| error::create_from_text(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    // exportName 须为字符串（不走 ToString）：非字符串抛 TypeError，不触发
    // 目标值的 toString 调用。
    let export_name_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let export_name = match vm.lookup_str(export_name_val) {
        Some(s) => s,
        None => {
            return NativeResult::Err(error::create_type_error(
                vm,
                "ShadowRealm.prototype.importValue exportName must be a string",
            ))
        }
    };

    // 建 Promise 能力（调用方 realm）。
    let (promise, _resolve, _reject) = vm.new_promise_capability();

    // 在影子 realm 内加载并求值模块，取导出值。
    let result = (|| -> Result<JsValue, JsValue> {
        // 解析模块路径（以当前工作目录为基准，test262 runner 以测试目录为工作目录）。
        let current_dir = std::env::current_dir()
            .map_err(|e| error::create_type_error(vm, &format!("cannot get current directory: {e}")))?;
        let module_path = current_dir.join(&specifier);
        let module_path = module_path
            .canonicalize()
            .map_err(|e| error::create_type_error(vm, &format!("cannot resolve module {specifier}: {e}")))?;
        let module_path_str = module_path.to_string_lossy().to_string();
        let source = std::fs::read_to_string(&module_path)
            .map_err(|e| error::create_type_error(vm, &format!("cannot read module {specifier}: {e}")))?;

        // 追加导出值提取语句（exportName 是模块作用域内的绑定名，追加语句可读）。
        let source_with_result = format!("{source}\nglobalThis.__oxide_import_result = {export_name};");

        // 编译并求值模块（语法错误 / 求值抛错 → TypeError）。
        let mut loader = ImportValueLoader;
        let module = {
            let shadow_vm = &mut vm.shadow_realm_objects.get_mut(&realm_id).unwrap().1;
            shadow_vm
                .compiler
                .compile_module(&source_with_result, &module_path_str, &mut loader)
        }
        .map_err(|msg| error::create_type_error(vm, &msg))?;
        {
            let shadow_vm = &mut vm.shadow_realm_objects.get_mut(&realm_id).unwrap().1;
            shadow_vm
                .run(&Arc::new(module))
                .map_err(|msg| error::create_type_error(vm, &msg))?;
        }

        // 读导出值（追加语句在模块求值末执行，导出绑定已初始化）。
        let export_value = {
            let shadow_vm = &mut vm.shadow_realm_objects.get_mut(&realm_id).unwrap().1;
            let global = shadow_vm.global_object();
            let global_ptr = global.as_ptr() as *mut JsObject;
            // SAFETY: global_ptr 是影子 realm 的 global 对象，存活。
            let global_obj = unsafe { &*global_ptr };
            let global_val = JsValue::from_js_object(global_ptr);
            let result_si = shadow_vm.perm_intern("__oxide_import_result");
            shadow_vm.ordinary_get(global_obj, result_si, global_val)
        }
        .map_err(|msg| error::create_type_error(vm, &msg))?;

        // 导出值结算：GetWrappedValue(callerRealm, exportValue)。
        settle_value(vm, vm.realm_id() as u64, realm_id, export_value)
    })();

    // 结算 Promise。
    match result {
        Ok(value) => {
            let _ = vm.resolve_promise(promise, value);
        }
        Err(e) => {
            let _ = vm.reject_promise(promise, e);
        }
    }
    NativeResult::Ok(promise)
}

/// WrappedFunction 的 native 函数项：结算 this 与实参、调用属主 realm 原函数、
/// 结算返回值。
///
/// # 步骤
/// 1. 从 callee 反查状态（原函数属主 realm 编号、保活表下标）。
/// 2. this 与实参逐个结算进目标 realm（可调用对象包装为内层 WrappedFunction、
///    非可调用对象抛 TypeError）。
/// 3. 调用原函数（经属主 Vm 路由，异常退出包 TypeError）。
/// 4. 返回值结算进调用方 realm。
fn wrapped_function_call(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    let callee_ptr = callee.as_js_object_ptr();
    let (target_realm_id, keepalive_index) = match vm.wrapped_function_state.get(&callee_ptr) {
        Some(state) => *state,
        None => {
            return NativeResult::Err(error::create_type_error(vm, "invalid WrappedFunction"));
        }
    };
    let caller_realm_id = vm.realm_id() as u64;

    // this 结算：GetWrappedValue(targetRealm, thisArgument)。native 调用协议把
    // receiver 寄存器下标放在实参表首位（`args[0]`，即 253 槽）。
    let this_val = vm.reg(args[0]);
    let settled_this = match settle_value(vm, target_realm_id, caller_realm_id, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };

    // 实参逐个结算：GetWrappedValue(targetRealm, arg)。
    let arg_count = vm.native_arg_count(args);
    let mut settled_args = Vec::with_capacity(arg_count);
    for i in 0..arg_count {
        let arg = vm.native_arg_at(args, i);
        match settle_value(vm, target_realm_id, caller_realm_id, arg) {
            Ok(v) => settled_args.push(v),
            Err(e) => return NativeResult::Err(e),
        }
    }

    // 调用原函数（经属主 Vm 路由，异常退出包 TypeError）。
    let result = {
        let target_vm = resolve_vm(vm, target_realm_id);
        let target_fn = target_vm.shadow_keepalive[keepalive_index];
        target_vm.call_function_sync(target_fn, settled_this, &settled_args)
    };
    let value = match result {
        Ok(v) => v,
        Err(_) => return NativeResult::Err(error::create_type_error(vm, "Error calling wrapped function")),
    };

    // 返回值结算：GetWrappedValue(callerRealm, result)。
    match settle_value(vm, caller_realm_id, target_realm_id, value) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// 按 realm 编号解析属主 Vm：自身、直接子影子 realm，或沿属主链上溯。
///
/// # 边界与前提
/// - `realm_id` 从 `vm` 可达（自身、子影子 realm 或属主链祖先）：状态表只登记
///   可达 realm 编号，前提恒成立。
/// - 目标 Vm 与 `vm` 同线程且非当前执行中（调用方在 `vm` 的 native 调用内），
///   返回的 `&mut` 安全。
fn resolve_vm(vm: &mut Vm, realm_id: u64) -> &mut Vm {
    if vm.realm_id() as u64 == realm_id {
        return vm;
    }
    if let Some(entry) = vm.shadow_realm_objects.get_mut(&realm_id) {
        return &mut entry.1;
    }
    // 沿属主链上溯（影子 Vm → 属主 Vm → … → 主 Vm）。
    let mut cur = vm.owner_vm;
    while !cur.is_null() {
        // SAFETY: 属主 Vm 与 vm 同线程存活且非当前执行中。
        let owner = unsafe { &mut *cur };
        if owner.realm_id() as u64 == realm_id {
            return owner;
        }
        if let Some(entry) = owner.shadow_realm_objects.get_mut(&realm_id) {
            return &mut entry.1;
        }
        cur = owner.owner_vm;
    }
    panic!("ShadowRealm: realm {realm_id} 不可从属主链解析")
}

/// 跨 realm 值结算：GetWrappedValue 语义（包装在 `wrapper_realm_id` realm，
/// 原函数属主在 `target_realm_id` realm）。
///
/// # 边界
/// - 可调用对象：包装为 `wrapper_realm_id` realm 的 WrappedFunction，原函数在
///   `target_realm_id` realm 的 Vm 保活表保活。
/// - 非可调用对象：抛 TypeError。
/// - 符号：按 realm 重映射（well-known 同下标、注册表符号按 key 复用 / 新建、
///   普通符号按描述新建）。
/// - 其余原始值：直返。
fn settle_value(vm: &mut Vm, wrapper_realm_id: u64, target_realm_id: u64, value: JsValue) -> Result<JsValue, JsValue> {
    if value.is_object() {
        if oxide_builtins::iterator::is_callable(value) {
            return settle_function(vm, wrapper_realm_id, target_realm_id, value);
        }
        return Err(error::create_type_error(vm, "ShadowRealm: cannot return non-primitive value"));
    }
    if value.is_symbol() {
        return settle_symbol(vm, wrapper_realm_id, target_realm_id, value);
    }
    Ok(value)
}

/// 函数结算：WrappedFunctionCreate(wrapper_realm, target)。
///
/// # 步骤
/// 1. 目标侧：原函数入属主 Vm 保活表、读 name / length（getter 抛错包 TypeError）。
/// 2. 包装侧：计算 name / length、建函数对象、登记状态表。
fn settle_function(
    vm: &mut Vm, wrapper_realm_id: u64, target_realm_id: u64, target_fn: JsValue,
) -> Result<JsValue, JsValue> {
    // 目标侧：保活原函数、读 name / length。
    let (keepalive_index, name_result, length_result, has_own_length) = {
        let target_vm = resolve_vm(vm, target_realm_id);
        let keepalive_index = target_vm.shadow_keepalive.len();
        target_vm.shadow_keepalive.push(target_fn);
        let target_ptr = target_fn.as_js_object_ptr();
        // SAFETY: target_fn 是属主 Vm 的函数对象，存活。
        let target_obj = unsafe { &*target_ptr };
        let name_si = target_vm.perm_intern("name");
        let length_si = target_vm.perm_intern("length");
        // HasOwnProperty 代理感知：getOwnPropertyDescriptor trap 可抛（撤销代理
        // 或抛错 trap），抛错包 TypeError（规范：CopyNameAndLength abrupt 抛 TypeError）。
        let has_own_length = match oxide_builtins::object::own_descriptor_of(target_vm, target_obj, length_si) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => return Err(error::create_type_error(vm, "ShadowRealm: error checking function length")),
        };
        let name_result = target_vm.ordinary_get(target_obj, name_si, target_fn);
        let length_result = if has_own_length {
            target_vm.ordinary_get(target_obj, length_si, target_fn)
        } else {
            Ok(JsValue::undefined())
        };
        (keepalive_index, name_result, length_result, has_own_length)
    };

    // getter 抛错包 TypeError（规范：WrappedFunctionCreate 从调用方 realm 抛 TypeError）。
    let target_name = match name_result {
        Ok(v) => v,
        Err(_) => return Err(error::create_type_error(vm, "ShadowRealm: error reading function name")),
    };
    let target_length = match length_result {
        Ok(v) => v,
        Err(_) => return Err(error::create_type_error(vm, "ShadowRealm: error reading function length")),
    };

    // 计算包装后的 name / length。
    let wrapped_name = {
        let wrapper_vm = resolve_vm(vm, wrapper_realm_id);
        wrapper_vm.lookup_str(target_name).unwrap_or_default()
    };
    let wrapped_length = compute_wrapped_length(target_length, has_own_length);

    // 包装侧：建 WrappedFunction。
    let wrapper_vm = resolve_vm(vm, wrapper_realm_id);
    Ok(create_wrapped_function(
        wrapper_vm,
        target_realm_id,
        keepalive_index,
        &wrapped_name,
        wrapped_length,
    ))
}

/// 符号结算：按 realm 重映射到包装 realm。
///
/// # 步骤
/// 1. 目标侧：读符号描述与注册表 key。
/// 2. 包装侧：well-known 按同下标、注册表符号按 key 复用 / 新建、
///    普通符号按描述新建（不入注册表）。
fn settle_symbol(vm: &mut Vm, wrapper_realm_id: u64, target_realm_id: u64, value: JsValue) -> Result<JsValue, JsValue> {
    let local_index = value.as_symbol_local_index();
    // 目标侧：读符号描述与注册表 key。
    let (description, registry_key) = {
        let target_vm = resolve_vm(vm, target_realm_id);
        (target_vm.symbol_description(local_index), target_vm.symbol_key_for_id(local_index))
    };

    // 包装侧：建 / 复用。
    let wrapper_vm = resolve_vm(vm, wrapper_realm_id);
    let wrapper_realm_id = wrapper_vm.realm_id();
    if local_index < WELL_KNOWN_SYMBOL_COUNT {
        // well-known 符号：跨 realm 同下标。
        return Ok(JsValue::symbol_realm(wrapper_realm_id, local_index));
    }
    if let Some(key) = registry_key {
        // 注册表符号：包装 realm 已有则复用，否则新建并登记。
        if let Some(idx) = wrapper_vm.symbol_lookup_global(&key) {
            return Ok(JsValue::symbol_realm(wrapper_realm_id, idx));
        }
        let idx = wrapper_vm.symbol_intern(description);
        wrapper_vm.symbol_register_global(key, idx);
        return Ok(JsValue::symbol_realm(wrapper_realm_id, idx));
    }
    // 普通符号：按描述新建（不入注册表）。
    let idx = wrapper_vm.symbol_intern(description);
    Ok(JsValue::symbol_realm(wrapper_realm_id, idx))
}

/// 建 WrappedFunction：包装 realm 的函数对象（native 函数项），登记状态表。
///
/// # 副作用
/// - 包装 realm 新建函数对象（session 分配）；状态表登记一条（GC 根）。
fn create_wrapped_function(
    vm: &mut Vm, target_realm_id: u64, keepalive_index: usize, name: &str, length: JsValue,
) -> JsValue {
    let native_fn: NativeFn = wrapped_function_call;
    let fn_proto = vm.realm.session.borrow().builtin_world().fn_proto_val();
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
    func.set_function(true);
    // SAFETY: wrapped_function_call 是 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn as *const ()) }));
    let ptr = vm.alloc_object(func);
    // SAFETY: alloc_object 返回非空 session 对象指针；借出期间无别名。
    let obj = unsafe { &mut *ptr };
    let attrs = PropAttributes::new(false, false, true);
    let length_si = vm.kernel_core.perm_interner().intern("length").0;
    let name_si = vm.kernel_core.perm_interner().intern("name").0;
    let _ = vm.define_data_property(obj, length_si, length, attrs);
    let name_val = vm.new_string(name);
    let _ = vm.define_data_property(obj, name_si, name_val, attrs);
    vm.wrapped_function_state.insert(ptr, (target_realm_id, keepalive_index));
    JsValue::from_js_object(ptr)
}

/// CopyNameAndLength 的 length 规则：无自身 length 归 0、非 Number 归 0、
/// +Infinity 归 +Infinity、-Infinity / NaN 归 0、有限数归 max(ToIntegerOrInfinity, 0)。
fn compute_wrapped_length(target_length: JsValue, has_own_length: bool) -> JsValue {
    if !has_own_length || !target_length.is_number() {
        return JsValue::float(0.0);
    }
    let n = oxide_runtime_api::to_number(target_length);
    if n == f64::INFINITY {
        return JsValue::float(f64::INFINITY);
    }
    if n == f64::NEG_INFINITY || n.is_nan() {
        return JsValue::float(0.0);
    }
    let len = to_integer_or_infinity(target_length);
    JsValue::float(len.max(0.0))
}

/// brand 校验：`this` 须是对象且指针在主 Vm 侧表内（实例对象注册表）。
fn shadow_realm_brand_check(vm: &mut Vm, this_val: JsValue) -> Result<(), JsValue> {
    if !this_val.is_object() {
        return Err(error::create_type_error(vm, "ShadowRealm method called on non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    let in_table = vm
        .shadow_realm_objects
        .values()
        .any(|(obj, _)| obj.as_js_object_ptr() == this_ptr);
    if !in_table {
        return Err(error::create_type_error(vm, "not a ShadowRealm instance"));
    }
    Ok(())
}

/// 据实例对象指针反查影子 realm 编号（brand 校验已保证存在）。
fn shadow_realm_id_of(vm: &Vm, this_ptr: *mut JsObject) -> u64 {
    vm.shadow_realm_objects
        .iter()
        .find(|(_, (obj, _))| obj.as_js_object_ptr() == this_ptr)
        .map(|(id, _)| *id)
        .unwrap()
}

/// 读全局 `ShadowRealm` 构造器的 `prototype` 属性（ShadowRealm.prototype）。
///
/// # 边界
/// - 全局 `ShadowRealm` 槽缺失或构造器无 `prototype` 属性时回退到
///   `%Object.prototype%`（防御性兜底，正常路径 `bind_shadow_realm` 已安装）。
fn shadow_realm_prototype_value(vm: &mut Vm) -> JsValue {
    let object_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ObjectProto));
    let global = vm.global_object();
    let global_ptr = global.as_ptr() as *mut JsObject;
    // SAFETY: global_ptr 是当前 session 的 global 对象，存活。
    let global_obj = unsafe { &*global_ptr };
    let si_shadow_realm = vm.perm_intern("ShadowRealm");
    let Some(store) = vm.get_own_property_slot(global_obj, si_shadow_realm) else {
        return object_proto;
    };
    let ctor_val = global_obj.get_prop_at(store);
    if !ctor_val.is_object() {
        return object_proto;
    }
    let ctor_ptr = ctor_val.as_js_object_ptr();
    // SAFETY: ctor_ptr 是 ShadowRealm 构造器对象，存活。
    let ctor_obj = unsafe { &*ctor_ptr };
    let si_prototype = vm.perm_intern("prototype");
    let Some(proto_store) = vm.get_own_property_slot(ctor_obj, si_prototype) else {
        return object_proto;
    };
    let proto_val = ctor_obj.get_prop_at(proto_store);
    if proto_val.is_object() {
        proto_val
    } else {
        object_proto
    }
}

/// importValue 的模块加载器：以 `base_dir` 为基准解析相对导入，读磁盘 fixture 源码。
struct ImportValueLoader;

impl oxide_runtime_api::ModuleSourceLoader for ImportValueLoader {
    fn resolve(&mut self, base_dir: &str, specifier: &str) -> Result<(String, String), String> {
        let base = std::path::Path::new(base_dir);
        let full = if specifier.starts_with('/') {
            std::path::PathBuf::from(specifier.trim_start_matches('/'))
        } else {
            base.join(specifier)
        };
        let canonical = full
            .canonicalize()
            .map_err(|e| format!("cannot resolve module {specifier}: {e}"))?;
        let content =
            std::fs::read_to_string(&canonical).map_err(|e| format!("cannot read module {specifier}: {e}"))?;
        Ok((content, canonical.to_string_lossy().to_string()))
    }
}
