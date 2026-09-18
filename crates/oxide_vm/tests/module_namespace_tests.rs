//! 模块命名空间 @@toStringTag Symbol 键与自身键枚举口径：
//! 命名空间标签以 well-known symbol 键写入，`Reflect.ownKeys` 的 Symbol 段跟在
//! 字符串段之后，字符串枚举面（getOwnPropertyNames / Object.keys）不受污染。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleKind, ModuleSourceLoader, ResolvedModule};
use oxide_vm::vm::Vm;

/// 文件加载器：base_dir/specifier join 后读源（依赖模块经真实文件解析）。
struct FileLoader;

impl ModuleSourceLoader for FileLoader {
    fn resolve(
        &mut self, base_dir: &str, specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        let joined = std::path::Path::new(base_dir).join(specifier);
        // 解析为规范绝对路径：自导入身份比较要求 loader 的 path 与入口模块的
        // 规范路径逐字对齐（emit 侧入口路径已 canonicalize）。
        let full = std::fs::canonicalize(&joined).map_err(|e| format!("cannot read {specifier}: {e}"))?;
        let source = std::fs::read_to_string(&full).map_err(|e| format!("cannot read {specifier}: {e}"))?;
        Ok(ResolvedModule {
            source,
            path: full.to_string_lossy().into_owned(),
            kind: ModuleKind::Js,
        })
    }
}

/// 测试结束删除临时目录，避免在 crate 树残留文件。
struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 同一 VM 续跑脚本并返回字符串完成值（读 global 探针用）。
fn probe_str(vm: &mut Vm, source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let value = vm.run(&Arc::new(module)).expect("probe run");
    vm.lookup_str(value).unwrap_or_default()
}

/// 编译并运行带依赖的模块，返回其探针字符串。
fn run_namespace_module(dir: &std::path::Path) -> String {
    let allocator = oxide_parser::Allocator::default();
    let source = std::fs::read_to_string(dir.join("main.mjs")).expect("read main");
    let program = oxide_parser::parse_module(&allocator, &source).expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, dir.join("main.mjs").to_string_lossy().as_ref(), &mut FileLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    probe_str(&mut vm, "globalThis.__ns")
}

#[test]
fn namespace_to_string_tag_symbol_key_and_own_keys() {
    // 命名空间标签按 Symbol.toStringTag Symbol 键暴露：描述符值/属性正确，
    // 字符串导出按 UTF-16 单元序排序，Symbol 段排在字符串段之后，
    // 字符串枚举面不含标签。
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("dep.mjs"), "export const b = 1;\nexport const a = 2;\nexport default 3;")
        .expect("write dep");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './dep.mjs';\n \
         var desc = Object.getOwnPropertyDescriptor(ns, Symbol.toStringTag);\n \
         globalThis.__ns = [\n \
           ns[Symbol.toStringTag],\n \
           typeof ns[Symbol.toStringTag],\n \
           Object.prototype.toString.call(ns),\n \
           Object.getOwnPropertySymbols(ns).length,\n \
           Object.getOwnPropertySymbols(ns)[0] === Symbol.toStringTag,\n \
           Object.getOwnPropertyNames(ns).length,\n \
           Object.getOwnPropertyNames(ns).join(','),\n \
           Reflect.ownKeys(ns).length,\n \
           Reflect.ownKeys(ns).indexOf(Symbol.toStringTag),\n \
           Symbol.toStringTag in ns,\n \
           Reflect.has(ns, Symbol.toStringTag),\n \
           Object.prototype.hasOwnProperty.call(ns, Symbol.toStringTag),\n \
           desc.writable,\n \
           desc.enumerable,\n \
           desc.configurable,\n \
           Object.getOwnPropertyNames(ns).indexOf('@@toStringTag'),\n \
           Object.keys(ns).length,\n \
         ].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(
        result, "Module|string|[object Module]|1|true|3|a,b,default|4|3|true|true|true|false|false|false|-1|3",
        "命名空间标签 Symbol 键与自身键枚举面不符"
    );
}

#[test]
fn namespace_exotic_semantics() {
    // 模块命名空间 exotic 四语义：创建起不可扩展、导出 writable:true、
    // [[Set]] 恒 false、[[DefineOwnProperty]] 收窄、Object.freeze 恒抛。
    // 用自导入在 body 内观测扩展性（此时 body 尚未执行到 seal）。
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_exotic__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         export const x = 1;\n \
         export default 2;\n \
         const d = Object.getOwnPropertyDescriptor(ns, 'x');\n \
         function threw(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } }\n \
         globalThis.__ns = [\n \
           Object.isExtensible(ns),\n \
           d.writable === true && d.enumerable === true && d.configurable === false,\n \
           Reflect.set(ns, 'x', 9),\n \
           threw(function () { ns.x = 9; }),\n \
           Reflect.set(ns, Symbol('s'), 1),\n \
           Reflect.defineProperty(ns, 'x', {}),\n \
           Reflect.defineProperty(ns, 'x', { value: 9 }),\n \
           threw(function () { Object.defineProperty(ns, 'x', { value: 9 }); }),\n \
           Reflect.defineProperty(ns, 'x', { writable: false }),\n \
           Reflect.defineProperty(ns, 'newKey', {}),\n \
           threw(function () { Object.freeze(ns); }),\n \
           Object.isFrozen(ns),\n \
           Object.seal(ns) === ns,\n \
           ns.default,\n \
         ].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(
        result, "false|true|false|true|false|true|false|true|false|false|true|false|true|2",
        "命名空间 exotic 语义不符"
    );
}

/// live 自导入 ns 的条目语义：lexical/class 导出预注册后初始化前读抛 ReferenceError，
/// `export var` 在实例化期即初始化为 undefined（读得 undefined 不抛）；`in` /
/// `Reflect.has` 已为 true；初始化后读到活值；枚举未初始化名不抛错且键数正确。
/// 循环读 `ns.y` 同时钉住 IC 慢路径与普通路径读结果一致。
#[test]
fn namespace_self_import_live_entries() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_live__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         const init = [];\n \
         for (const k of ['x', 'y', 'default']) {\n \
           try { ns[k]; init.push('value'); } catch (e) { init.push(e instanceof ReferenceError); }\n \
         }\n \
         const hasBefore = ['x', 'y', 'default'].map(k => k in ns).join(',');\n \
         const namesBefore = Object.getOwnPropertyNames(ns).join(',');\n \
         const ownKeysCount = Reflect.ownKeys(ns).length;\n \
         export let x = 'sx';\n \
         export var y = 2;\n \
         export default 3;\n \
         let sum = 0;\n \
         for (let i = 0; i < 128; i++) { sum += ns.y; }\n \
         globalThis.__ns = [\n \
           init.join('|'),\n \
           hasBefore,\n \
           namesBefore,\n \
           ownKeysCount,\n \
           ns.x,\n \
           sum,\n \
           ns.default,\n \
         ].join(';');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(
        result, "true|value|true;true,true,true;default,x,y;4;sx;256;3",
        "live 自导入 ns 的未初始化读 / var 预初始化 / 键存在性 / 活值读不符"
    );
}

/// 未初始化导出在枚举面与描述符面经 `? [[GetOwnProperty]]` 抛 ReferenceError；
/// `[[OwnPropertyKeys]]` 面（getOwnPropertyNames / Reflect.ownKeys）不读值故不抛。
#[test]
fn namespace_uninitialized_enumeration_throws_reference_error() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_uninit_enum__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         const threw = fn => { try { fn(); return false; } catch (e) { return e instanceof ReferenceError; } };\n \
         const names = Object.getOwnPropertyNames(ns).join(',');\n \
         const ownKeysCount = Reflect.ownKeys(ns).length;\n \
         globalThis.__ns = [\n \
           threw(() => Object.keys(ns)),\n \
           threw(() => Object.values(ns)),\n \
           threw(() => Object.entries(ns)),\n \
           threw(() => Object.assign({}, ns)),\n \
           threw(() => Object.prototype.hasOwnProperty.call(ns, 'default')),\n \
           threw(() => Object.getOwnPropertyDescriptor(ns, 'default')),\n \
           threw(() => Object.prototype.propertyIsEnumerable.call(ns, 'default')),\n \
           threw(() => { for (const k in ns) {} }),\n \
           names,\n \
           ownKeysCount,\n \
         ].join('|');\n \
         export default 0;",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(
        result, "true|true|true|true|true|true|true|true|default|2",
        "未初始化导出的枚举/描述符抛错或 ownKeys 反向守卫不符"
    );
}

/// 顶层对导出源绑定的重赋写穿命名空间：一个源绑定背两个导出名时同值更新，
/// 复合赋值路径与简单赋值路径一致。
#[test]
fn namespace_assignment_write_through_multiple_export_names() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_write_through__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         export let a = 1;\n \
         export { a as b };\n \
         export let c = 1;\n \
         a = 2;\n \
         c += 2;\n \
         globalThis.__ns = [ns.a, ns.b, ns.c].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "2|2|3", "导出源绑定重赋未写穿命名空间活值");
}

/// 写穿以解析后的绑定身份为准：块级/catch 同名遮蔽的赋值不得污染模块导出。
/// 遮蔽名解析到内层绑定槽，与导出源绑定槽不同，写穿须按槽位过滤而非仅按名字。
#[test]
fn namespace_write_through_respects_block_shadowing() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_shadow__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         export let x = 1;\n \
         { let x = 2; x = 3; }\n \
         try { throw 0; } catch (x) { x = 4; }\n \
         globalThis.__ns = [ns.x, x].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "1|1", "块级/catch 同名遮蔽的赋值不得写穿命名空间条目");
}

/// 写穿覆盖 Update 表达式、逻辑赋值与解构赋值三条发射路径，与简单/二元复合
/// 赋值一致地把新值同步到命名空间条目。
#[test]
fn namespace_write_through_covers_update_logical_destructuring() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_write_paths__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         export let a = 1;\n \
         export let b = 0;\n \
         export let c = 1;\n \
         a++;\n \
         b ||= 7;\n \
         [c] = [9];\n \
         globalThis.__ns = [ns.a, ns.b, ns.c].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "2|7|9", "Update/逻辑/解构赋值的写穿缺失");
}

/// `export var` 属 VarScopedDeclarations，模块实例化期即初始化为 undefined：
/// body 语句执行前经命名空间读应得 undefined，而非 ReferenceError；`in` 已为 true。
#[test]
fn namespace_export_var_preinitialized() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_var_preinit__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './main.mjs';\n \
         let before;\n \
         try { before = ns.y; } catch (e) { before = 'threw'; }\n \
         globalThis.__ns = [String(before), 'y' in ns].join('|');\n \
         export var y = 2;",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "undefined|true", "export var 应在实例化期预初始化为 undefined");
}

/// G2 反向守卫：外部导入的命名空间无条目表（非 live），枚举与 for-in 行为不变。
#[test]
fn external_namespace_enumeration_stays_unaffected() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_namespace_external__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("dep.mjs"), "export const z = 1;").expect("write dep");
    std::fs::write(
        dir.join("main.mjs"),
        "import * as ns from './dep.mjs';\n \
         let n = 0;\n \
         for (const k in ns) n++;\n \
         globalThis.__ns = [Object.keys(ns).join(','), n, Object.values(ns).join(',')].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "z|1|1", "非 live 外部命名空间枚举行为不应变化");
}

/// 跨模块可重赋导出活读：依赖模块经闭包重赋源绑定后，导入方命名绑定读到新值。
#[test]
fn cross_module_named_import_reads_live_value() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_cross_live__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("dep.mjs"),
        "var x = 1;\nexport { x };\nglobalThis.__bump = function () { x = 2; };",
    )
    .expect("write dep");
    std::fs::write(
        dir.join("main.mjs"),
        "import { x } from './dep.mjs';\n \
         const before = x;\n \
         globalThis.__bump();\n \
         const after = x;\n \
         globalThis.__ns = [before, after].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "1|2", "命名导入未跟随依赖源绑定重赋");
}

/// 一源两别名：同一导出的两个导入局部名经活读同步更新，且互不分裂。
#[test]
fn cross_module_aliased_imports_read_live_value() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_cross_live_alias__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("dep.mjs"),
        "var x = 1;\nexport { x };\nglobalThis.__bump = function () { x = 2; };",
    )
    .expect("write dep");
    std::fs::write(
        dir.join("main.mjs"),
        "import { x as y, x as z } from './dep.mjs';\n \
         const before = [y, z];\n \
         globalThis.__bump();\n \
         globalThis.__ns = [before[0], before[1], y, z].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "1|1|2|2", "一源两别名未同步读到重赋后的活值");
}

/// 默认导出具名函数体的自引用重赋：函数体内 `fn = 2` 写模块级绑定，
/// 导入方再次读 default 得新值（含捕获分析下探 export 声明与共享 cell）。
#[test]
fn cross_module_default_export_reads_live_value() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_cross_live_default__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("dep.mjs"), "export default function fn() { fn = 2; return 1; }").expect("write dep");
    std::fs::write(
        dir.join("main.mjs"),
        "import val from './dep.mjs';\n \
         const ret = val();\n \
         globalThis.__ns = [ret, val].join('|');",
    )
    .expect("write main");
    let _cleanup = Cleanup(dir.clone());

    let result = run_namespace_module(&dir);
    assert_eq!(result, "1|2", "default 导出的具名函数自引用重赋未活读");
}
