use oxide_runtime_api::NativeResult;

/// native 内置函数的函数指针类型。
///
/// `args` 是寄存器下标列表（参数值取自 `Vm` 的寄存器）；返回 [`NativeResult`]
/// 表示成功/抛错/尾调用三种结局。绑定层通过 `fn` 项指针构造，保证零开销调用。
pub type NativeFn = fn(&mut crate::vm::Vm, args: &[u8]) -> NativeResult;
