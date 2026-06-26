use crate::level::Level;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubsystemId {
    Vm = 0,
    Ic = 1,
    Kernel = 2,
    Builtins = 3,
}

pub const SUBSYSTEM_COUNT: usize = 4;

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
