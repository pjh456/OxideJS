//! test262 宿主对象 `$262` 的最小实现。
//!
//! `$262` 是 test262 套件约定的宿主注入对象（非 ECMAScript 标准全局）：
//! 测试经它访问当前 realm 全局、执行脚本、触发 GC 等。最小绑定使引用
//! `$262.*` 的测试真实执行而非一律抛 `ReferenceError` 被 runner 吞为 skip；
//! 未实现的方法抛 `not supported` 错误，runner 按能力缺失归类。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_units_full, NativeResult};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::{encode_symbol_key, WELL_KNOWN_SYMBOL_TO_STRING_TAG};
use oxide_types::value::JsValue;

use crate::bindings::{apply_binding_table, bind_accessor_getter_key, bind_global_value, BindEnv};
use crate::vm::Vm;

/// `$262` 未实现方法统一抛错：消息带 `not supported`，runner 据此按能力缺失跳过。
fn not_supported(vm: &mut Vm, feature: &str) -> NativeResult {
    NativeResult::Err(oxide_builtins::error::create_type_error(vm, &format!("{feature} is not supported")))
}

/// `$262.evalScript(code)`：在当前 realm 编译执行一段普通脚本并返回其完成值。
///
/// 按 test262 harness 规范 evalScript 即当前 realm 的 script（非 eval）：
/// 顶层 var/function 声明落全局对象，完成值为末条语句的正常完成值。
///
/// # 步骤
/// 1. 实参 ToString 取脚本源码。
/// 2. 经普通脚本动态编译（`create_plain_dynamic_script` + 同步调用）。
/// 3. 编译失败转 SyntaxError；运行异常原样透传异常值。
///
/// # 边界与前提
/// - 普通脚本语义：var/函数声明落全局对象（configurable:false），let/const
///   落全局词法环境，完成值保留（`evalScript('var y = 1; y')` → 1）。
/// - 无实参或非字符串实参按 ToString 规范处理。
pub fn eval_script(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let units = match to_units_full(vm.reg(args[1]), vm) {
        Ok(u) => u,
        Err(e) => return NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
    };
    // 动态路径源契约：源码域转义形态传源（见 `create_dynamic_function`）。
    let code = oxide_kernel::string_forge::source_escape(&units);
    match vm.create_plain_dynamic_script(&code) {
        Ok(func) => {
            let global = JsValue::from_js_object(vm.realm.session.borrow().global_object().as_ptr() as *mut JsObject);
            match vm.call_function_sync(func, global, &[]) {
                Ok(val) => NativeResult::Ok(val),
                Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
            }
        }
        Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &format!("SyntaxError: {e}"))),
    }
}

/// `$262.detachArrayBuffer(buf)`：真 detach——品牌守卫后载荷 `data → None`，
/// 返回 undefined。
pub fn detach_array_buffer(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    oxide_builtins::array_buffer::detach_array_buffer_native::<Vm>(vm, this_val)
}

/// `$262.createRealm()`：跨 realm 未支持，抛能力缺失错误。
pub fn create_realm(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    not_supported(vm, "createRealm")
}

/// `$262.gc()`：请求一次 session 全量强制收集，返回 undefined。
///
/// 收集延迟到下一个顶层指令边界执行，不在 native 重入点就地跑：重入期间
/// dispatch 调用链上的调用方寄存器窗口副本与 callee 引用是 Rust 局部值
/// （非 GC 根），移动式 sweep 就地搬移会使它们悬垂，返回后恢复动作还会
/// 把陈旧指针写回 `regs[254]`。顶层指令边界是无在途局部值的安全点，
/// 收集经既有完整收集入口（与引擎正常触发路径同一函数）执行。
/// 对 JS 可观察语义等价于同步：收集在下一条指令求值前完成。
///
/// # 副作用
/// - 置强制收集旗标；下一个顶层指令边界执行 mark + 移动式 sweep +
///   字符串/BigInt 清扫，重写 session 对象指针。
pub fn gc(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    vm.realm.gc.borrow_mut().pending_forced_collect = true;
    NativeResult::Ok(JsValue::undefined())
}

/// `$262.agent`：agent 线程接口依赖 SharedArrayBuffer 族，未支持，抛能力缺失错误。
pub fn agent(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    not_supported(vm, "agent")
}

/// `$262.IsHTMLDDA(...)`：B.3.4 宿主对象本体，被调用时返回 null。
///
/// 规范定义无参或首参空串时返回 null（String 方法、迭代器协议经 GetMethod
/// 读得后按可调用值调用即得 null）；其余调用形态行为未定义，统一返回 null。
pub fn is_html_dda_call(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::null())
}

/// 构造 `%AbstractModuleSource%` 内建（source-phase 导入的 ModuleSource 实例族）：
/// 构造器调用恒抛 TypeError，原型带 `constructor` 与 `@@toStringTag` 访问器。
///
/// # 步骤
/// 1. 建原型对象（Object.prototype 为原型）：`constructor` 指回构造器
///    （可写不可枚举可配置），`@@toStringTag` 装原生 getter（不可枚举可配置）。
/// 2. 建构造器函数对象（Function.prototype 为原型，可构造，原生入口抛 TypeError）：
///    `prototype` 指回原型（不可写不可枚举不可配置）、`name` = "AbstractModuleSource"、
///    `length` = 0（均不可写不可枚举可配置）。
/// 3. 原型与构造器登记进 world 释放表（session 收尾统一释放）。
///
/// # 注意事项
/// - 每次绑定新建对象（与 `$262` 本体同一约定）：选择性重建后旧内建对象随宿主
///   对象泄漏，全量初始化与 full_reset 频率低，可接受。
fn build_abstract_module_source_ctor(env: BindEnv<'_>) -> *mut JsObject {
    let sf = env.core.perm_interner().as_ref();
    let sh = env.core.shape_forge().as_ref();
    let world = env.world();

    // 原型对象：constructor 槽先装（getter 安装会再扩 shape 槽）。
    let object_proto = world.object_proto.as_ptr() as *mut JsObject;
    let proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let proto_ptr = Box::into_raw(proto);

    // 构造器函数对象：原生入口恒抛 TypeError（规范 28.1.1.1 不可构造）。
    let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
    let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    ctor.set_function(true);
    ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
    ctor.set_native_fn(Some(unsafe {
        NativeFnPtr::from_raw(oxide_builtins::module::abstract_module_source_ctor::<crate::vm::Vm> as *const ())
    }));
    ctor.set_native_arg_count(0);

    // 属性面：prototype / name / length 三槽（描述符按规范内建函数约定）。
    let si_prototype = sf.intern("prototype").0;
    let si_name = sf.intern("name").0;
    let si_length = sf.intern("length").0;
    let shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let shape2 = sh.make_shape(shape1, si_name);
    let shape3 = sh.make_shape(shape2, si_length);
    ctor.set_shape_id(shape3);
    ctor.ensure_hash_props().push(JsValue::from_js_object(proto_ptr));
    ctor.ensure_hash_props()
        .push(JsValue::perm_string(sf.string_ptr(sf.intern("AbstractModuleSource").0)));
    ctor.ensure_hash_props().push(JsValue::int(0));
    ctor.set_data_meta(0, PropAttributes::new(false, false, false));
    ctor.set_data_meta(1, PropAttributes::new(false, false, true));
    ctor.set_data_meta(2, PropAttributes::new(false, false, true));
    let ctor_ptr = Box::into_raw(ctor);

    // 原型 constructor 槽：可写不可枚举可配置（与一般内建原型约定一致）。
    let proto_ref = unsafe { &mut *proto_ptr };
    let si_constructor = sf.intern("constructor").0;
    let ctor_shape = sh.make_shape(proto_ref.shape_id(), si_constructor);
    proto_ref.set_shape_id(ctor_shape);
    proto_ref.ensure_hash_props().push(JsValue::from_js_object(ctor_ptr));
    let ctor_pos = proto_ref.prop_vec_len().saturating_sub(1) as u32;
    proto_ref.set_data_meta(ctor_pos, PropAttributes::new(true, false, true));
    proto_ref.bump_generation();

    // 原型 @@toStringTag 访问器：getter 按实例类名标记返回值，非实例返 undefined。
    let tag_key = encode_symbol_key(env.realm_id(), WELL_KNOWN_SYMBOL_TO_STRING_TAG);
    bind_accessor_getter_key(
        env,
        proto_ref,
        tag_key,
        "get [Symbol.toStringTag]",
        oxide_builtins::module::abstract_module_source_to_string_tag::<crate::vm::Vm> as *const (),
    );

    world.track_leaked_object(ctor_ptr);
    world.track_leaked_object(proto_ptr);
    ctor_ptr
}

/// 把 `$262` 宿主对象绑定到 global：对象本体 + 标准方法 + `global` 数据属性。
///
/// # 步骤
/// 1. 建 `$262` 普通对象（Object.prototype）。
/// 2. 绑定 evalScript/detachArrayBuffer/createRealm/gc/agent 方法。
/// 3. `global` 数据属性指向当前 session global；`$262` 本体挂到 global。
/// 4. `IsHTMLDDA` 宿主对象挂到 `$262`（可调用，被调用时返回 null）。
/// 5. `AbstractModuleSource` 内建挂到 `$262`（source-phase 导入的 ModuleSource 实例族）。
///
/// # 注意事项
/// - 宿主对象登记进 world 释放表，session 收尾时统一释放（与 Reflect/Iterator
///   等全局对象同一约定）；每次调用新建对象，同一 session 内重复绑定会泄漏旧对象
///   （全量初始化与 full_reset 频率低，可接受）。
pub fn bind_test262_host(env: BindEnv<'_>, global: &mut JsObject) {
    let object_proto = env.world().object_proto.as_ptr() as *mut JsObject;
    let mut host = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    apply_binding_table(
        env,
        &mut host,
        &[
            ("evalScript", eval_script as *const (), 1),
            ("detachArrayBuffer", detach_array_buffer as *const (), 1),
            ("createRealm", create_realm as *const (), 0),
            ("gc", gc as *const (), 0),
            ("agent", agent as *const (), 0),
        ],
    );
    let global_this = JsValue::from_js_object(global as *mut JsObject);
    bind_global_value(env.core, &mut host, "global", global_this);
    // %AbstractModuleSource% 内建：source-phase 导入的 ModuleSource 实例族
    // （构造器调用抛 TypeError，原型承载 @@toStringTag 访问器）。
    let amms_ctor = build_abstract_module_source_ctor(env);
    bind_global_value(env.core, &mut host, "AbstractModuleSource", JsValue::from_js_object(amms_ctor));
    // [[IsHTMLDDA]] 宿主值（B.3.4）：可调用（被调用时返回 null），
    // ToBoolean/typeof/宽松相等按 undefined 处理，Type 与其余强制转换按普通对象处理。
    let mut dda = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    dda.type_tag = JsObject::OBJ_TYPE_HTML_DDA;
    dda.set_function(true);
    dda.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(is_html_dda_call as *const ()) }));
    dda.set_native_arg_count(0);
    let dda_ptr = Box::into_raw(Box::new(dda));
    env.world().track_leaked_object(dda_ptr);
    bind_global_value(env.core, &mut host, "IsHTMLDDA", JsValue::from_js_object(dda_ptr));
    let host_ptr = Box::into_raw(Box::new(host));
    env.world().track_leaked_object(host_ptr);
    bind_global_value(env.core, global, "$262", JsValue::from_js_object(host_ptr));
}
