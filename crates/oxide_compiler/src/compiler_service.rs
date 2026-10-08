//! 默认编译服务：串联 `oxide_parser::parse` 与 `Compiler::compile`，
//! parse 错误映射与 `Allocator` 构造的唯一实现源。

use oxide_bytecode::module::CompiledModule;
use oxide_parser::Program;
use oxide_runtime_api::CompilerService;

use crate::compiler::Compiler;

/// 无状态默认编译服务：两方法均无内部可变状态，
/// `Arc<dyn CompilerService>` 可跨线程共享。
pub struct DefaultCompilerService;

impl CompilerService for DefaultCompilerService {
    fn compile_script(&self, source: &str) -> Result<CompiledModule, String> {
        // 局部分配器：AST 节点经 oxc bump 分配器分配，编译完成即弃。
        let allocator = oxide_parser::Allocator::default();
        let program = parse_program(&allocator, source)?;
        // 动态路径源契约：调用方以源码域转义（`string_forge::source_escape`）
        // 形态传源，故置 source_encoded。
        Compiler::new().with_source_encoded(true).compile(&program)
    }

    fn compile_eval_script(&self, source: &str) -> Result<CompiledModule, String> {
        let allocator = oxide_parser::Allocator::default();
        let program = parse_program(&allocator, source)?;
        // eval 脚本：顶层 var/function 声明落全局属性 configurable:true。
        Compiler::new()
            .with_eval_script(true)
            .with_source_encoded(true)
            .compile(&program)
    }
}

/// 解析源码，解析错误各条消息换行拼接为单一错误串。
fn parse_program<'a>(allocator: &'a oxide_parser::Allocator, source: &'a str) -> Result<Program<'a>, String> {
    oxide_parser::parse(allocator, source)
        .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))
}
