use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    Compiler::new().compile(&program).expect("compile failed")
}


