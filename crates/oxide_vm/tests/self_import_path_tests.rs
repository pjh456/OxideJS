//! 自导入路径口径：顶层模块相对发现路径 vs 加载器 resolve 的 canonicalize 绝对
//! 路径，二者须同口径，自导入才按自身命名空间绑定、整模块只求值一次。
//! 误判为外部依赖会把自导入重新编译求值一遍，模块被求值两次。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleKind, ModuleSourceLoader, ResolvedModule};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

/// 自导入模块源：`import * as self from './selfm.js'` 导入自身，每次求值给
/// global 计数加一，导出 self 命名空间引用以实化自导入绑定。
const MODULE_SRC: &str = "import * as self from './selfm.js';\n \
                          globalThis.__evals = (globalThis.__evals || 0) + 1;\n \
                          export const self_ref = self;";

/// 复刻 test262 加载器：base_dir/specifier join 后 canonicalize 为绝对规范路径
/// 并读源——与顶层相对发现路径构成「绝对 vs 相对」口径差，正是本面复现条件。
struct AbsLoader;

impl ModuleSourceLoader for AbsLoader {
    fn resolve(
        &mut self, base_dir: &str, specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        let full = std::path::Path::new(base_dir).join(specifier);
        let canonical = full.canonicalize().map_err(|e| format!("cannot resolve {specifier}: {e}"))?;
        let source = std::fs::read_to_string(&canonical).map_err(|e| format!("cannot read {specifier}: {e}"))?;
        Ok(ResolvedModule {
            source,
            path: canonical.to_string_lossy().into_owned(),
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

/// 同一 VM 续跑脚本并返回完成值（读 global 计数用）。
fn probe(vm: &mut Vm, source: &str) -> JsValue {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("probe run")
}

#[test]
fn self_import_module_evaluated_once() {
    // 临时模块落在 CWD 相对路径下，使顶层发现路径可为相对形式、canonicalize
    // 又可达——与 runner「相对发现路径 + 绝对 resolve」的口径差同构。
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__selfimport_repro__");
    let file = dir.join("selfm.js");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(&file, MODULE_SRC).expect("write module");
    let _cleanup = Cleanup(dir.clone());

    let allocator = oxide_parser::Allocator::default();
    let source = std::fs::read_to_string(&file).expect("read module");
    let program = oxide_parser::parse_module(&allocator, &source).expect("parse module");
    // 顶层模块路径用相对发现路径（不 canonicalize），复刻 runner 侧口径。
    let module = Compiler::new()
        .compile_module(&program, "__selfimport_repro__/selfm.js", &mut AbsLoader)
        .expect("compile module");

    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    // 自导入须绑定自身命名空间，整模块只求值一次；误判为外部依赖会二次求值。
    let r = probe(&mut vm, "globalThis.__evals");
    assert!(r.is_int() && r.as_int() == 1, "自导入模块应只求值一次，实际 __evals={r:?}");
}

/// 自导入绑定是源绑定的活引用：闭包经别名读到的必须是源绑定的当前值；
/// 源在导出声明后重赋时，别名读点同步可见新值（共享 cell）。
#[test]
fn self_import_alias_reads_live_source_value() {
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__selfimport_live__");
    let file = dir.join("live.mjs");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        &file,
        "let before;\n \
         function readAlias() { return y; }\n \
         try { before = readAlias(); } catch (e) { before = e instanceof ReferenceError; }\n \
         import { x as y } from './live.mjs';\n \
         export let x = 1;\n \
         x = 2;\n \
         globalThis.__live = [before, readAlias()].join('|');",
    )
    .expect("write module");
    let _cleanup = Cleanup(dir.clone());

    let allocator = oxide_parser::Allocator::default();
    let source = std::fs::read_to_string(&file).expect("read module");
    let program = oxide_parser::parse_module(&allocator, &source).expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, dir.join("live.mjs").to_string_lossy().as_ref(), &mut AbsLoader)
        .expect("compile module");

    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    // 声明前闭包读别名按规范抛 ReferenceError；声明后重赋读到新值 2。
    let r = probe(&mut vm, "globalThis.__live");
    let text = vm.lookup_str(r).unwrap_or_default();
    assert_eq!(text, "true|2", "自导入别名应共享源绑定活值");
}
