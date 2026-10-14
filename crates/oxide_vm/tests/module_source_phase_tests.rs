//! source-phase 导入（`import source x from`）的端到端语义钉：局部名绑定
//! %AbstractModuleSource% 实例、`<module source>` 宿主哨兵说明符解析、再导出链
//! 与 star 转发的 `~source~` 来源身份（同源同绑定不歧义）、`$262.AbstractModuleSource`
//! 内建面（构造器抛 TypeError、原型描述符、`@@toStringTag` getter 行为）。

use std::collections::HashMap;
use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleKind, ModuleSourceLoader, ResolvedModule};
use oxide_vm::vm::Vm;

/// 内存版加载器：specifier 精确查表；`<module source>` 哨兵说明符解析为合成空模块
/// （与 test262 runner 的宿主口径一致），ModuleSource 对象由宿主在绑定时提供。
struct MemSourceLoader {
    table: HashMap<String, String>,
}

impl MemSourceLoader {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self {
            table: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }
}

impl ModuleSourceLoader for MemSourceLoader {
    fn resolve(
        &mut self, _base_dir: &str, specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        if specifier == "<module source>" {
            return Ok(ResolvedModule {
                source: String::new(),
                path: "<module source>".to_string(),
                kind: ModuleKind::Js,
            });
        }
        self.table
            .get(specifier)
            .map(|source| ResolvedModule {
                source: source.clone(),
                path: specifier.to_string(),
                kind: ModuleKind::Js,
            })
            .ok_or_else(|| format!("MemSourceLoader 无 {specifier}"))
    }
}

/// 编译并运行模块，返回探针字符串（探针脚本接在同一 VM 上续跑）。
fn run_module(main: &str, deps: &[(&str, &str)], probe: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, main).expect("parse module");
    let mut loader = MemSourceLoader::new(deps);
    let module = Compiler::new()
        .compile_module(&program, "main.mjs", &mut loader)
        .expect("compile module");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    let allocator = oxide_parser::Allocator::default();
    let probe_prog = oxide_parser::parse(&allocator, probe).expect("parse probe");
    let probe_module = Compiler::new().compile(&probe_prog).expect("compile probe");
    let value = vm.run(&Arc::new(probe_module)).expect("probe run");
    vm.lookup_str(value).unwrap_or_default()
}

/// 面 1：`import source x from '<module source>'` 局部名绑定 ModuleSource 对象——
/// 对象形态、`instanceof $262.AbstractModuleSource` 成立。
#[test]
fn import_source_binding_is_module_source_object() {
    let r = run_module(
        "import source x from '<module source>';\n \
         globalThis.__t = [typeof x, x instanceof $262.AbstractModuleSource].join(',');",
        &[],
        "globalThis.__t",
    );
    assert_eq!(r, "object,true", "source-phase 局部名应绑定对象且为 %AbstractModuleSource% 实例");
}

/// 面 2：再导出链（fixture `import source x; export { x }`）——命名导入与命名空间
/// [[Get]] 均取目标模块的 ModuleSource 对象。
#[test]
fn reexported_source_binding_named_import_and_namespace_get() {
    let r = run_module(
        "import { x } from './fixture.js';\n \
         import * as ns from './fixture.js';\n \
         globalThis.__t = [x instanceof $262.AbstractModuleSource, ns.x === x].join(',');",
        &[("./fixture.js", "import source x from '<module source>'; export { x };")],
        "globalThis.__t",
    );
    assert_eq!(r, "true,true", "再导出链的命名导入与命名空间读应得同一 ModuleSource 对象");
}

/// 面 3：两 fixture 各 `import source mod from '<module source>'; export { mod }`，
/// 经 star 转发再导出——同源同绑定（`~source~` 身份相同）不歧义，导入方取到对象。
#[test]
fn star_reexport_of_source_binding_is_unambiguous() {
    let r = run_module(
        "import { mod } from './reexport.js';\n \
         globalThis.__t = [typeof mod, mod instanceof $262.AbstractModuleSource].join(',');",
        &[
            ("./reexport.js", "export * from './f1.js'; export * from './f2.js';"),
            ("./f1.js", "import source mod from '<module source>'; export { mod };"),
            ("./f2.js", "import source mod from '<module source>'; export { mod };"),
        ],
        "globalThis.__t",
    );
    assert_eq!(r, "object,true", "同源 source 绑定经 star 转发应不歧义且解析为 ModuleSource");
}

/// 面 4：`$262.AbstractModuleSource` 内建面——构造器调用抛 TypeError、
/// name/length 描述符、prototype 不可写不可配置。
#[test]
fn abstract_module_source_intrinsic_surface() {
    let r = run_module(
        "var C = $262.AbstractModuleSource;\n \
         var threw = false;\n \
         try { new C(); } catch (e) { threw = e instanceof TypeError; }\n \
         var d = Object.getOwnPropertyDescriptor(C, 'prototype');\n \
         globalThis.__t = [threw, C.name, C.length, d.writable, d.configurable].join(',');",
        &[],
        "globalThis.__t",
    );
    assert_eq!(
        r, "true,AbstractModuleSource,0,false,false",
        "构造器应抛 TypeError、name/length/prototype 描述符按规范"
    );
}

/// 面 5：`@@toStringTag` getter 行为——实例返回类名串，非实例（数字 / 原型本体）
/// 返回 undefined。
#[test]
fn abstract_module_source_to_string_tag_getter() {
    let r = run_module(
        "import source x from '<module source>';\n \
         var C = $262.AbstractModuleSource;\n \
         var get = Object.getOwnPropertyDescriptor(C.prototype, Symbol.toStringTag).get;\n \
         globalThis.__t = [String(x[Symbol.toStringTag]), String(get.call(262)), String(get.call(C.prototype))].join(',');",
        &[],
        "globalThis.__t",
    );
    assert_eq!(
        r, "AbstractModuleSource,undefined,undefined",
        "实例类名串可读，非实例 getter 返 undefined"
    );
}
