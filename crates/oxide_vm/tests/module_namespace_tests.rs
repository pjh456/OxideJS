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

/// live 自导入 ns 的条目语义：预注册后初始化前读抛 ReferenceError，`in` /
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
        result, "true|true|true;true,true,true;default,x,y;4;sx;256;3",
        "live 自导入 ns 的未初始化读 / 键存在性 / 活值读不符"
    );
}
