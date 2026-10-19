//! 默认编译服务：串联 `oxide_parser::parse` 与 `Compiler::compile`，
//! parse 错误映射的唯一实现源。

use oxide_bytecode::module::CompiledModule;
use oxide_parser::Program;
use oxide_runtime_api::{CompilerService, ModuleSourceLoader};

use crate::compiler::Compiler;

/// 无状态默认编译服务：三方法均无内部可变状态，
/// `Arc<dyn CompilerService>` 可跨线程共享。
pub struct DefaultCompilerService;

impl CompilerService for DefaultCompilerService {
    fn compile_script(&self, source: &str) -> Result<CompiledModule, String> {
        // 线程本地分配器：AST 节点经 oxc bump 分配器分配，编译完成即弃。
        oxide_parser::with_parse_allocator(|allocator| {
            let program = parse_program(allocator, source)?;
            // 动态路径源契约：调用方以源码域转义（`string_forge::source_escape`）
            // 形态传源，故置 source_encoded。
            Compiler::new().with_source_encoded(true).compile(&program)
        })
    }

    fn compile_eval_script(&self, source: &str) -> Result<CompiledModule, String> {
        oxide_parser::with_parse_allocator(|allocator| {
            let program = parse_program(allocator, source)?;
            // eval 代码是脚本：import/export 声明不得出现（规范 Script 产生式），
            // 按模块解析命中即 SyntaxError（既有包装臂转成 JS 异常）。
            if program.source_type.is_module() {
                return Err("eval code may not contain import or export declarations".to_string());
            }
            // eval 脚本：顶层 var/function 声明落全局属性 configurable:true。
            Compiler::new()
                .with_eval_script(true)
                .with_source_encoded(true)
                .compile(&program)
        })
    }

    fn compile_module(
        &self, source: &str, module_path: &str, loader: &mut dyn ModuleSourceLoader,
    ) -> Result<CompiledModule, String> {
        // 模块源按磁盘文件读入（非 source_encoded 形态），故不设 source_encoded。
        oxide_parser::with_parse_allocator(|allocator| {
            let program = parse_module_program(allocator, source)?;
            // 适配 runtime_api 加载器到 emit 层加载器（依赖解析回调）。
            let mut emit_loader = EmitLoaderAdapter { inner: loader };
            Compiler::new().compile_module(&program, module_path, &mut emit_loader)
        })
    }
}

/// 按 ES module 文法解析源码，解析错误各条消息换行拼接为单一错误串。
fn parse_module_program<'a>(allocator: &'a oxide_parser::Allocator, source: &'a str) -> Result<Program<'a>, String> {
    oxide_parser::parse_module(allocator, source)
        .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))
}

/// 把 `runtime_api` 的模块加载器适配为 `oxide_emit` 的加载器：
/// 依赖解析回调的桥接，种类恒为 JS（数据模块种类由 emit 层按需判定）。
struct EmitLoaderAdapter<'a> {
    inner: &'a mut dyn ModuleSourceLoader,
}

impl oxide_emit::module::ModuleSourceLoader for EmitLoaderAdapter<'_> {
    fn resolve(
        &mut self, base_dir: &str, specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<oxide_emit::module::ResolvedModule, String> {
        let (source, path) = self.inner.resolve(base_dir, specifier)?;
        Ok(oxide_emit::module::ResolvedModule {
            source,
            path,
            kind: oxide_emit::module::ModuleKind::Js,
        })
    }
}

/// 解析源码，解析错误各条消息换行拼接为单一错误串。
fn parse_program<'a>(allocator: &'a oxide_parser::Allocator, source: &'a str) -> Result<Program<'a>, String> {
    oxide_parser::parse(allocator, source)
        .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))
}
