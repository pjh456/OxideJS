//! forge 状态查询：读四张共享 forge 的条目数与容量，执行三个行动旗标
//! （gc / clear-cache / lookup）。
//!
//! `handle_forge_query` 是纯函数，参数全注入、无全局状态；worker 线程调用
//! （gc 需触及本线程自有池），forge 状态读是跨线程安全的只读操作。

use std::sync::Arc;

use oxide_kernel::kernel::KernelCore;
use oxide_vm::vm_pool::VmPool;

use super::protocol::{ForgeTarget, LookupResult, ServerResponse};

/// forge 查询处理：读目标 forge 状态，按旗标执行 gc / clear-cache / lookup，
/// 组装 ForgeStatus 响应帧。
///
/// # 步骤
/// 1. 读目标 forge 条目数与容量（code 目标容量取 max_cached_modules，其余 0）。
/// 2. --gc 时对池内全体空闲 VM 执行完整 session GC，记收集数。
/// 3. --clear-cache 时清空字节码 LRU 缓存。
/// 4. --lookup 时在字符串 intern 表纯查询键的 id（不插入）。
/// 5. 组装 ForgeStatus 响应帧返回。
///
/// # 边界与前提
/// - 在 worker 线程调用（gc 需触及本线程自有池）；forge 状态读是跨线程安全
///   的只读操作。
/// - gc 只触及空闲队列，在借 VM（在途 eval）不受影响；clear-cache 清 LRU
///   不使在借模块失效（Arc 保活）。
///
/// # 副作用
/// - --gc 重置各空闲 VM 的 session_bytes_allocated 为清扫后存活字节。
/// - --clear-cache 清空字节码缓存（后续 eval 重编译）。
pub fn handle_forge_query(
    target: ForgeTarget, gc: bool, clear_cache: bool, lookup: Option<String>, kernel: &Arc<KernelCore>,
    pool: &Arc<VmPool>,
) -> ServerResponse {
    // 读目标 forge 条目数与容量。
    let (entries, capacity) = match target {
        ForgeTarget::Code => (kernel.code_forge().len(), kernel.max_cached_modules()),
        ForgeTarget::Object => (kernel.shape_forge().len(), 0),
        ForgeTarget::String => (kernel.perm_interner().entry_count() as usize, 0),
        ForgeTarget::Property => (kernel.prop_forge().len(), 0),
    };

    // --gc：全体空闲 VM 完整 session GC。
    let gc_collected = gc.then(|| pool.collect_idle_gc());

    // --clear-cache：清空字节码 LRU。
    let cache_cleared = if clear_cache {
        kernel.code_forge().clear();
        true
    } else {
        false
    };

    // --lookup：字符串 intern 表纯查询（不插入）。
    let lookup_result = lookup.map(|key| match kernel.perm_interner().lookup_id(&key) {
        Some(id) => LookupResult::Interned { id },
        None => LookupResult::Absent,
    });

    ServerResponse::ForgeStatus {
        target,
        entries,
        capacity,
        gc_collected,
        cache_cleared,
        lookup: lookup_result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::eval;
    use oxide_kernel::kernel::KernelConfig;

    /// 测试环境：minimal 内核加单 VM 池（与 eval 测试同口径）。
    fn test_env() -> (Arc<KernelCore>, Arc<VmPool>) {
        let kernel = KernelCore::new(KernelConfig::minimal());
        let pool = VmPool::new(Arc::clone(&kernel), 1, Some(2));
        (kernel, pool)
    }

    /// code 目标：容量等于 max_cached_modules，条目数 0（新内核），无旗标无行动结果。
    #[test]
    fn forge_query_code_entries() {
        let (kernel, pool) = test_env();
        let response = handle_forge_query(ForgeTarget::Code, false, false, None, &kernel, &pool);
        match response {
            ServerResponse::ForgeStatus {
                target,
                entries,
                capacity,
                gc_collected,
                cache_cleared,
                lookup,
            } => {
                assert_eq!(target, ForgeTarget::Code, "目标应为 code");
                assert_eq!(entries, 0, "新内核的字节码缓存应为空");
                assert_eq!(capacity, kernel.max_cached_modules(), "容量应等于 max_cached_modules");
                assert!(gc_collected.is_none(), "未设 gc 旗标应无收集数");
                assert!(!cache_cleared, "未设 clear-cache 旗标不应清缓存");
                assert!(lookup.is_none(), "未设 lookup 旗标应无结果");
            }
            other => panic!("应得 ForgeStatus 帧，实得 {other:?}"),
        }
    }

    /// gc 旗标：先 eval 制造 session 状态归还池，再 gc 得收集数 Some(1)。
    #[test]
    fn forge_query_gc_collects() {
        let (kernel, pool) = test_env();
        eval::handle_eval("1 + 1", None, &kernel, &pool);
        let response = handle_forge_query(ForgeTarget::Code, true, false, None, &kernel, &pool);
        match response {
            ServerResponse::ForgeStatus { gc_collected, .. } => {
                assert_eq!(gc_collected, Some(1), "单 VM 池应收集 1 个空闲 VM");
            }
            other => panic!("应得 ForgeStatus 帧，实得 {other:?}"),
        }
    }

    /// lookup：已 intern 键得 Interned，未 intern 键得 Absent，且纯查询不插入。
    #[test]
    fn forge_query_lookup() {
        let (kernel, pool) = test_env();
        let (id, _) = kernel.perm_interner().intern("forge-key");
        let before = kernel.perm_interner().entry_count();

        let response = handle_forge_query(ForgeTarget::String, false, false, Some("forge-key".into()), &kernel, &pool);
        match response {
            ServerResponse::ForgeStatus { lookup, .. } => {
                assert_eq!(lookup, Some(LookupResult::Interned { id }), "已 intern 键应得该 id");
            }
            other => panic!("应得 ForgeStatus 帧，实得 {other:?}"),
        }

        let response =
            handle_forge_query(ForgeTarget::String, false, false, Some("not-interned".into()), &kernel, &pool);
        match response {
            ServerResponse::ForgeStatus { lookup, .. } => {
                assert_eq!(lookup, Some(LookupResult::Absent), "未 intern 键应得 Absent");
            }
            other => panic!("应得 ForgeStatus 帧，实得 {other:?}"),
        }

        assert_eq!(kernel.perm_interner().entry_count(), before, "纯查询不应插入新条目");
    }
}
