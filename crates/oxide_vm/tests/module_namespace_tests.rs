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
        let full = std::path::Path::new(base_dir).join(specifier);
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
