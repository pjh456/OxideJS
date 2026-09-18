//! star 再导出的同名冲突省略：第三方观察者模块导入 hub 命名空间，验证歧义名
//! 在 `in` / `Object.keys` / for-in 三面一致省略，同绑定重复提供与显式导出不误删。
//!
//! 观察者形态（hub 不自导入）与自别名回写解耦：本文件只钉省/留判定本身。

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

/// 编译并运行带依赖的模块，返回 `main.mjs` 写入 `globalThis.__r` 的探针字符串。
fn run_observer_module(dir: &std::path::Path) -> String {
    let allocator = oxide_parser::Allocator::default();
    let source = std::fs::read_to_string(dir.join("main.mjs")).expect("read main");
    let program = oxide_parser::parse_module(&allocator, &source).expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, dir.join("main.mjs").to_string_lossy().as_ref(), &mut FileLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    probe_str(&mut vm, "globalThis.__r")
}

/// 建目录并写入给定文件（文件名, 源码）。
fn fixture_dir(tag: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join(format!("__module_star_{tag}__"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (name, source) in files {
        std::fs::write(dir.join(name), source).expect("write fixture");
    }
    dir
}

/// 双星源各自本地声明同名 `both`：歧义名从命名空间省略，`in` / `Object.keys` /
/// for-in 三面一致；唯一名与显式导出照常保留。
#[test]
fn star_ambiguous_local_binding_is_omitted() {
    let dir = fixture_dir(
        "ambiguous",
        &[
            ("s1.mjs", "export var first = 'a';\nexport var both = 1;"),
            ("s2.mjs", "export var second = 'b';\nexport var both = 2;"),
            ("hub.mjs", "export * from './s1.mjs';\nexport * from './s2.mjs';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n \
                 let forks = 0;\n \
                 for (const k in ns) forks += (k === 'both' ? 10 : 1);\n \
                 globalThis.__r = [\n \
                   'first' in ns,\n \
                   'second' in ns,\n \
                   'both' in ns,\n \
                   ns.both === undefined,\n \
                   Object.keys(ns).join(','),\n \
                   Object.getOwnPropertyNames(ns).includes('both'),\n \
                   forks,\n \
                   ns.first,\n \
                   ns.second,\n \
                 ].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(
        run_observer_module(&dir),
        "true|true|false|true|first,second|false|2|a|b",
        "歧义名应从命名空间三面一致省略，唯一名保留"
    );
}

/// 双星源经 `export * as foo from` 转发同一命名空间对象：同 `(模块, 绑定)` 保留。
#[test]
fn star_same_namespace_reexport_binding_is_kept() {
    let dir = fixture_dir(
        "star_as",
        &[
            ("empty.mjs", ""),
            ("s1.mjs", "export * as foo from './empty.mjs';"),
            ("s2.mjs", "export * as foo from './empty.mjs';"),
            ("hub.mjs", "export * from './s1.mjs';\nexport * from './s2.mjs';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [typeof ns.foo, 'foo' in ns].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "object|true", "同命名空间再导出不应被误删");
}

/// 双星源经 `import * as foo; export { foo }` 转发同一命名空间对象：同绑定保留。
#[test]
fn star_same_namespace_import_reexport_binding_is_kept() {
    let dir = fixture_dir(
        "import_star_as",
        &[
            ("empty.mjs", ""),
            ("s1.mjs", "import * as foo from './empty.mjs';\nexport { foo };"),
            ("s2.mjs", "import * as foo from './empty.mjs';\nexport { foo };"),
            ("hub.mjs", "export * from './s1.mjs';\nexport * from './s2.mjs';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [typeof ns.foo, 'foo' in ns].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "object|true", "导入命名空间再导出不应被误删");
}

/// 双星源经 `export { foo } from base` / `import { foo } from base; export { foo }`
/// 转发同一绑定：`(模块, 绑定)` 相同，保留。
#[test]
fn star_same_named_reexport_binding_is_kept() {
    let dir = fixture_dir(
        "propagates",
        &[
            ("base.mjs", "export const foo = 2;"),
            ("s1.mjs", "export { foo } from './base.mjs';"),
            ("s2.mjs", "import { foo } from './base.mjs';\nexport { foo };"),
            ("hub.mjs", "export * from './s1.mjs';\nexport * from './s2.mjs';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [ns.foo, 'foo' in ns].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "2|true", "同源绑定经不同路径转发不应被误删");
}

/// 三源：S1/S2 本地同名歧义后，S3 与 S1 同模块（同绑定 token）也不得重新加入。
#[test]
fn star_ambiguous_stays_omitted_with_third_source() {
    let dir = fixture_dir(
        "three_sources",
        &[
            ("s1.mjs", "export var x = 1;\nexport var keep = 's1';"),
            ("s2.mjs", "export var x = 2;"),
            (
                "hub.mjs",
                "export * from './s1.mjs';\nexport * from './s2.mjs';\nexport * from './s1.mjs';",
            ),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = ['x' in ns, ns.keep].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "false|s1", "一旦歧义不得因后续同绑定源重新加入");
}

/// 显式本地导出在 star 歧义省略之后执行：显式导出重新定义该名并胜出。
#[test]
fn star_ambiguous_then_local_export_revives_name() {
    let dir = fixture_dir(
        "revive",
        &[
            ("s1.mjs", "export var both = 1;"),
            ("s2.mjs", "export var both = 2;"),
            ("hub.mjs", "export * from './s1.mjs';\nexport * from './s2.mjs';\nexport var both = 'local';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [ns.both, 'both' in ns, Object.keys(ns).join(',')].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "local|true|both", "star 歧义后显式导出应重新定义并胜出");
}

/// 显式本地导出与 star 源同名：无论语句先后，显式导出恒胜。
#[test]
fn star_local_export_wins_over_star_source() {
    let dir = fixture_dir(
        "local_first",
        &[
            ("dep.mjs", "export var x = 'dep';\nexport var d = 1;"),
            ("hub.mjs", "export var x = 'local';\nexport * from './dep.mjs';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [ns.x, 'x' in ns, ns.d].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "local|true|1", "本地导出先于 star 时恒胜");

    let dir = fixture_dir(
        "local_last",
        &[
            ("dep.mjs", "export var x = 'dep';\nexport var d = 1;"),
            ("hub.mjs", "export * from './dep.mjs';\nexport var x = 'local';"),
            (
                "main.mjs",
                "import * as ns from './hub.mjs';\n globalThis.__r = [ns.x, 'x' in ns, ns.d].join('|');",
            ),
        ],
    );
    let _cleanup = Cleanup(dir.clone());
    assert_eq!(run_observer_module(&dir), "local|true|1", "本地导出后于 star 时覆盖 star 值");
}
