use std::sync::Arc;

use oxide_kernel::kernel::KernelCore;

pub fn register_global_properties(_kernel: &Arc<KernelCore>, _bump: &bumpalo::Bump) -> Result<(), String> {
    Ok(())
}
