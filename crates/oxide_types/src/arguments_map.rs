//! mapped arguments 对象同步状态盒：参数寄存器与 arguments 对象存储值双向同步的
//! 创建期帧身份与逐索引映射存活位图。

/// mapped arguments 对象同步状态盒（存于 `native_data`，无 GC 边）。
///
/// 承载参数寄存器基址、形参数、逐索引映射存活位图与创建期帧身份，供参数寄存器
/// 与 arguments 对象存储值的双向同步（exotic `[[Get]]`/`[[Set]]`/`[[DefineOwnProperty]]`/
/// `[[Delete]]` 四臂共用）。
pub struct ArgumentsMapState {
    /// 首形参寄存器下标（`CompiledModule.param_base`）。
    pub param_base: u8,
    /// 形参数（`[[ParameterMap]]`，`CompiledModule.n_args`）。
    pub param_count: u16,
    /// 逐索引映射存活位图：bit i = 1 表示索引 i 仍映射。长度 = ceil(param_count / 64)，
    /// 构造期全 1（尾部多余位清零）。
    pub mapped_mask: Vec<u64>,
    /// 创建帧在帧栈中的下标（创建期 `Vm.frames.len() - 1`）。
    pub frame_depth: u32,
    /// 创建期帧身份（单调递增 `frame_id`）。
    pub frame_id: u64,
}

impl ArgumentsMapState {
    /// 索引 `index` 的映射是否存活（位图对应位为 1）。
    pub fn is_mapped(&self, index: u16) -> bool {
        let word = (index as usize) / 64;
        let bit = (index as usize) % 64;
        word < self.mapped_mask.len() && (self.mapped_mask[word] >> bit) & 1 == 1
    }

    /// 移除索引 `index` 的映射（清位）。
    pub fn unmap(&mut self, index: u16) {
        let word = (index as usize) / 64;
        let bit = (index as usize) % 64;
        if word < self.mapped_mask.len() {
            self.mapped_mask[word] &= !(1u64 << bit);
        }
    }
}
