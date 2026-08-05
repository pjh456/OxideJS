//! 子系统标识与日志级别解析。
//!
//! 引擎日志按子系统划分（VM / IC / Kernel / Builtins），各子系统可独立设置
//! 级别。`parse_level` 把字符串解析为 [`Level`]；`apply_env_levels` 读取
//! `OXIDE_LOG` 环境变量并按 `oxide=<level>` 形式的指令批量覆盖级别。

use crate::level::Level;

/// 引擎子系统标识。
///
/// `repr(usize)` 的值同时用作 [`SubsystemFilter`](crate::SubsystemFilter)
/// 内部原子级别数组的下标，故枚举顺序不可随意调整。
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubsystemId {
    /// 虚拟机执行循环（字节码解释器）。
    Vm = 0,
    /// 内联缓存（inline cache）相关路径。
    Ic = 1,
    /// 内核：对象模型、GC、运行时服务。
    Kernel = 2,
    /// 内置对象与内置函数实现。
    Builtins = 3,
}

/// 子系统总数，即 [`SubsystemId`] 变体个数，也是 `SubsystemFilter` 级别数组长度。
pub const SUBSYSTEM_COUNT: usize = 4;

/// 把级别字符串解析为 [`Level`]，大小写不敏感。
///
/// 接受 `off` / `error` / `warn` / `info` / `debug` / `trace`，未知字符串返回
/// `None`。
pub fn parse_level(s: &str) -> Option<Level> {
    if s.eq_ignore_ascii_case("off") {
        Some(Level::Off)
    } else if s.eq_ignore_ascii_case("error") {
        Some(Level::Error)
    } else if s.eq_ignore_ascii_case("warn") {
        Some(Level::Warn)
    } else if s.eq_ignore_ascii_case("info") {
        Some(Level::Info)
    } else if s.eq_ignore_ascii_case("debug") {
        Some(Level::Debug)
    } else if s.eq_ignore_ascii_case("trace") {
        Some(Level::Trace)
    } else {
        None
    }
}

/// 从环境变量 `OXIDE_LOG` 应用启动时级别覆盖。
///
/// 指令格式为逗号分隔的 `target=level` 列表，如 `oxide::vm=debug,oxide::ic=info`；
/// `oxide=<level>` 为所有子系统的快捷写法。无效指令被静默跳过，
/// 若环境变量缺失则直接返回。
pub fn apply_env_levels() {
    let Ok(spec) = std::env::var("OXIDE_LOG") else {
        return;
    };
    for directive in spec.split(',') {
        let Some((target, level_str)) = directive.split_once('=') else {
            continue;
        };
        let Some(level) = parse_level(level_str.trim()) else {
            continue;
        };
        match target.trim() {
            "oxide" => {
                crate::set_level(SubsystemId::Vm, level);
                crate::set_level(SubsystemId::Ic, level);
                crate::set_level(SubsystemId::Kernel, level);
                crate::set_level(SubsystemId::Builtins, level);
            }
            "oxide::vm" => crate::set_level(SubsystemId::Vm, level),
            "oxide::ic" => crate::set_level(SubsystemId::Ic, level),
            "oxide::kernel" => crate::set_level(SubsystemId::Kernel, level),
            "oxide::builtins" => crate::set_level(SubsystemId::Builtins, level),
            _ => {}
        }
    }
}
