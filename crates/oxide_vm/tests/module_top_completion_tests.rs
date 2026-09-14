//! 模块顶层完成值口径：顶层模块求值完成值为空记录（run 返回 undefined），
//! 依赖模块仍经 __moduleEval 以返回值作命名空间对象（contract 不可破）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleKind, ModuleSourceLoader, ResolvedModule};
use oxide_types::value::JsValue;
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

/// 同一 VM 续跑脚本并返回完成值（读 global 探针用）。
fn probe(vm: &mut Vm, source: &str) -> JsValue {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("probe run")
}

#[test]
fn top_level_module_run_returns_undefined() {
    // 顶层模块求值完成值按规范为空记录：对外表现 undefined，不再是命名空间对象。
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, "export const a = 1; 5").expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, "test.mjs", &mut FileLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("module run");
    assert!(result.is_undefined(), "顶层模块 run 完成值应为 undefined，实际 {result:?}");
}

#[test]
fn top_level_module_without_exports_run_returns_undefined() {
    // 无导出顶层模块（空命名空间）同口径：run 返 undefined。
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, "const x = 42;").expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, "test.mjs", &mut FileLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("module run");
    assert!(result.is_undefined(), "顶层模块 run 完成值应为 undefined，实际 {result:?}");
}

#[test]
fn dependent_module_namespace_contract_intact() {
    // 依赖模块返回值仍被 __moduleEval 作命名空间消费：导入绑定值经依赖
    // 命名空间正确流入顶层模块（顶层 run 完成值归 undefined 不波及依赖面）。
    let cwd = std::env::current_dir().expect("cwd");
    let dir = cwd.join("__module_top_completion__");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("dep.mjs"), "export const v = 7; export function f() { return 'f'; }").expect("write dep");
    std::fs::write(
        dir.join("top.mjs"),
        "import { v, f } from './dep.mjs';\n \
         globalThis.__dep = v === 7 && f() === 'f';\n \
         export const w = 9;",
    )
    .expect("write top");
    let _cleanup = Cleanup(dir.clone());

    let allocator = oxide_parser::Allocator::default();
    let source = std::fs::read_to_string(dir.join("top.mjs")).expect("read top");
    let program = oxide_parser::parse_module(&allocator, &source).expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, dir.join("top.mjs").to_string_lossy().as_ref(), &mut FileLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");
    // 依赖命名空间 contract：导入值经依赖求值返回值正确流入顶层模块作用域。
    let r = probe(&mut vm, "globalThis.__dep === true");
    assert!(r.is_bool() && r.as_bool(), "依赖命名空间值应正确流入，实际 {r:?}");
}
