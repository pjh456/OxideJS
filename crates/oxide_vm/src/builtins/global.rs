use std::sync::Arc;

use oxide_kernel::kernel::OxideKernel;

pub fn register_global_properties(
    _kernel: &Arc<OxideKernel>,
    _bump: &bumpalo::Bump,
) -> Result<(), String> {
    Ok(())
}
