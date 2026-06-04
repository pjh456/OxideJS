use std::fmt;

/// OpCode for register-based bytecode VM.
///
/// Organized in groups of 16 for readability. Implemented opcodes have
/// emitter support in the compiler; placeholder opcodes are reserved
/// for future phases (IC, profiling, parallelization).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    // ── Arithmetic (0x00-0x0F) ──
    ADD = 0x00,
    SUB = 0x01,
    MUL = 0x02,
    DIV = 0x03,
    MOD = 0x04,
    NEG = 0x05,

    // ── Comparison (0x10-0x1F) ──
    EQ = 0x10,
    NEQ = 0x11,
    LT = 0x12,
    GT = 0x13,
    LTE = 0x14,
    GTE = 0x15,

    // ── Control Flow (0x20-0x2F) ──
    JMP = 0x20,
    JMP_IF_FALSE = 0x21,
    JMP_IF_TRUE = 0x22,

    // ── Variable (0x30-0x3F) ──
    LOAD_VAR = 0x30,
    STORE_VAR = 0x31,
    LOAD_CONST = 0x32,

    // ── Call (0x40-0x4F) ──
    CALL = 0x40,
    RETURN = 0x41,

    // ── Object Property — placeholders (0x50-0x5F) ──
    IC_GET_PROP = 0x50,
    IC_SET_PROP = 0x51,

    // ── Profiling — placeholders (0x60-0x6F) ──
    PROFILE_TYPE = 0x60,
    PROFILE_SHAPE = 0x61,
    PROFILE_BRANCH = 0x62,
    PROFILE_CALL = 0x63,

    // ── Parallel — placeholders (0x70-0x7F) ──
    FORK = 0x70,
    JOIN = 0x71,

    // ── Misc (0xF0-0xFF) ──
    NOP = 0xF0,
    HALT = 0xF1,
}

impl OpCode {
    pub fn is_implemented(&self) -> bool {
        !matches!(
            self,
            OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::PROFILE_TYPE
                | OpCode::PROFILE_SHAPE
                | OpCode::PROFILE_BRANCH
                | OpCode::PROFILE_CALL
                | OpCode::FORK
                | OpCode::JOIN
        )
    }
}

impl TryFrom<u8> for OpCode {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(OpCode::ADD),
            0x01 => Ok(OpCode::SUB),
            0x02 => Ok(OpCode::MUL),
            0x03 => Ok(OpCode::DIV),
            0x04 => Ok(OpCode::MOD),
            0x05 => Ok(OpCode::NEG),
            0x10 => Ok(OpCode::EQ),
            0x11 => Ok(OpCode::NEQ),
            0x12 => Ok(OpCode::LT),
            0x13 => Ok(OpCode::GT),
            0x14 => Ok(OpCode::LTE),
            0x15 => Ok(OpCode::GTE),
            0x20 => Ok(OpCode::JMP),
            0x21 => Ok(OpCode::JMP_IF_FALSE),
            0x22 => Ok(OpCode::JMP_IF_TRUE),
            0x30 => Ok(OpCode::LOAD_VAR),
            0x31 => Ok(OpCode::STORE_VAR),
            0x32 => Ok(OpCode::LOAD_CONST),
            0x40 => Ok(OpCode::CALL),
            0x41 => Ok(OpCode::RETURN),
            0x50 => Ok(OpCode::IC_GET_PROP),
            0x51 => Ok(OpCode::IC_SET_PROP),
            0x60 => Ok(OpCode::PROFILE_TYPE),
            0x61 => Ok(OpCode::PROFILE_SHAPE),
            0x62 => Ok(OpCode::PROFILE_BRANCH),
            0x63 => Ok(OpCode::PROFILE_CALL),
            0x70 => Ok(OpCode::FORK),
            0x71 => Ok(OpCode::JOIN),
            0xF0 => Ok(OpCode::NOP),
            0xF1 => Ok(OpCode::HALT),
            _ => Err(()),
        }
    }
}

impl fmt::Display for OpCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            OpCode::ADD => "ADD",
            OpCode::SUB => "SUB",
            OpCode::MUL => "MUL",
            OpCode::DIV => "DIV",
            OpCode::MOD => "MOD",
            OpCode::NEG => "NEG",
            OpCode::EQ => "EQ",
            OpCode::NEQ => "NEQ",
            OpCode::LT => "LT",
            OpCode::GT => "GT",
            OpCode::LTE => "LTE",
            OpCode::GTE => "GTE",
            OpCode::JMP => "JMP",
            OpCode::JMP_IF_FALSE => "JMP_IF_FALSE",
            OpCode::JMP_IF_TRUE => "JMP_IF_TRUE",
            OpCode::LOAD_VAR => "LOAD_VAR",
            OpCode::STORE_VAR => "STORE_VAR",
            OpCode::LOAD_CONST => "LOAD_CONST",
            OpCode::CALL => "CALL",
            OpCode::RETURN => "RETURN",
            OpCode::IC_GET_PROP => "IC_GET_PROP",
            OpCode::IC_SET_PROP => "IC_SET_PROP",
            OpCode::PROFILE_TYPE => "PROFILE_TYPE",
            OpCode::PROFILE_SHAPE => "PROFILE_SHAPE",
            OpCode::PROFILE_BRANCH => "PROFILE_BRANCH",
            OpCode::PROFILE_CALL => "PROFILE_CALL",
            OpCode::FORK => "FORK",
            OpCode::JOIN => "JOIN",
            OpCode::NOP => "NOP",
            OpCode::HALT => "HALT",
        };
        write!(f, "{name}")
    }
}
