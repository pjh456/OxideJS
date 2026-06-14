use std::fmt;

/// OpCode for register-based bytecode VM.
///
/// Organized in groups of 16 for readability. Implemented opcodes have
/// emitter support in the compiler; placeholder opcodes are reserved
/// for future phases (IC, profiling, parallelization).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum OpCode {
    // ── Arithmetic (0x00-0x0F) ──
    ADD = 0x00,
    SUB = 0x01,
    MUL = 0x02,
    DIV = 0x03,
    MOD = 0x04,
    NEG = 0x05,
    COMPOUND_ADD = 0x06,
    COMPOUND_SUB = 0x07,
    COMPOUND_MUL = 0x08,
    COMPOUND_DIV = 0x09,
    COMPOUND_MOD = 0x0A,
    COMPOUND_EXP = 0x0B,

    // -- Comparison (0x10-0x1F) --
    EQ = 0x10,
    NEQ = 0x11,
    LT = 0x12,
    GT = 0x13,
    LTE = 0x14,
    GTE = 0x15,
    IN = 0x16,
    NOT = 0x17,
    AND = 0x18,
    OR = 0x19,
    STRICT_EQ = 0x1A,
    STRICT_NEQ = 0x1C,
    UNARY_PLUS = 0x1D,

    // -- Control Flow (0x20-0x2F) --
    JMP = 0x20,
    JMP_IF_FALSE = 0x21,
    JMP_IF_TRUE = 0x22,
    FOR_OF_INIT = 0x23,
    FOR_OF_NEXT = 0x24,

    // -- Update (0x25-0x28) --
    INC_PRE = 0x25,
    INC_POST = 0x26,
    DEC_PRE = 0x27,
    DEC_POST = 0x28,

    FOR_IN_INIT = 0x29,
    FOR_IN_NEXT = 0x2A,
    FOR_IN_DONE = 0x2B,
    SWITCH_TABLE = 0x2C,
    FOR_IN_CLEANUP = 0x2D,

    // -- Exception (0x2E-0x2F, 0x33-0x35) --
    THROW = 0x2E,
    TRY_BEGIN = 0x2F,
    TRY_END = 0x33,
    TRY_FINALLY_BEGIN = 0x34,
    TRY_FINALLY_END = 0x35,
    FOR_OF_DONE = 0x36,
    FOR_OF_CLOSE = 0x37,

    // -- Template Literal (0x38) --
    TEMPLATE_STR = 0x38,

    // -- Small Language Features (0x39-0x3B) --
    DELETE_PROP_STATIC = 0x39,
    DELETE_PROP_DYNAMIC = 0x3A,
    INSTANCEOF = 0x3B,
    REST_OBJECT = 0x3E,

    // -- Variable (0x30-0x32) --
    LOAD_VAR = 0x30,
    STORE_VAR = 0x31,
    LOAD_CONST = 0x32,

    // -- Call (0x40-0x4F) --
    CALL = 0x40,
    RETURN = 0x41,
    CALL_NATIVE = 0x42,
    NEW_EXPRESSION = 0x43,
    SUPER_CALL = 0x44,
    SUPER_GET_PROP = 0x45,
    SUPER_STATIC_GET_PROP = 0x46,
    SET_HOME_OBJECT = 0x47,
    DEFINE_ACCESSOR = 0x48,

    // ── Object Property (0x50-0x5F) ──
    IC_GET_PROP = 0x50,
    IC_SET_PROP = 0x51,
    GET_PROP = 0x52,
    GET_PROP_DYNAMIC = 0x53,
    SET_PROP = 0x54,
    SET_PROP_DYNAMIC = 0x55,
    NEW_OBJECT = 0x56,
    NEW_ARRAY = 0x57,
    SET_ELEM = 0x58,

    // -- Member Update (0x59-0x62) --
    MEMBER_INC = 0x59,
    MEMBER_DEC = 0x5A,
    DYN_MEMBER_INC = 0x5B,
    DYN_MEMBER_DEC = 0x5C,
    COMPOUND_MEMBER_ADD = 0x5D,
    COMPOUND_MEMBER_SUB = 0x5E,
    COMPOUND_MEMBER_MUL = 0x5F,
    COMPOUND_MEMBER_DIV = 0x60,
    COMPOUND_MEMBER_MOD = 0x61,
    COMPOUND_MEMBER_EXP = 0x62,

    // -- Profiling - placeholders (0x63-0x6F) --
    PROFILE_TYPE = 0x63,
    PROFILE_SHAPE = 0x64,
    PROFILE_BRANCH = 0x65,
    PROFILE_CALL = 0x66,

    // ── Parallel — placeholders (0x70-0x7F) ──
    FORK = 0x70,
    JOIN = 0x71,

    // -- Bitwise (0x80-0x8F) --
    BIT_AND = 0x80,
    BIT_OR = 0x81,
    BIT_XOR = 0x82,
    SHL = 0x83,
    SHR = 0x84,
    USHR = 0x85,
    BIT_NOT = 0x86,
    COMPOUND_AND = 0x88,
    COMPOUND_OR = 0x89,
    COMPOUND_XOR = 0x8A,
    COMPOUND_SHL = 0x8B,
    COMPOUND_SHR = 0x8C,
    COMPOUND_USHR = 0x8D,

    // ── Misc (0xF0-0xFF) ──
    NOP = 0xF0,
    HALT = 0xF1,
    TYPEOF = 0xF2,
    VOID = 0xF3,
}

impl OpCode {
    pub fn is_implemented(&self) -> bool {
        !matches!(
            self,
            OpCode::PROFILE_TYPE
                | OpCode::PROFILE_SHAPE
                | OpCode::PROFILE_BRANCH
                | OpCode::PROFILE_CALL
                | OpCode::FORK
                | OpCode::JOIN
        )
    }

    pub fn has_ic_ext_words(&self) -> bool {
        matches!(
            self,
            OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::MEMBER_INC
                | OpCode::MEMBER_DEC
                | OpCode::COMPOUND_MEMBER_ADD
                | OpCode::COMPOUND_MEMBER_SUB
                | OpCode::COMPOUND_MEMBER_MUL
                | OpCode::COMPOUND_MEMBER_DIV
                | OpCode::COMPOUND_MEMBER_MOD
                | OpCode::COMPOUND_MEMBER_EXP
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
            0x06 => Ok(OpCode::COMPOUND_ADD),
            0x07 => Ok(OpCode::COMPOUND_SUB),
            0x08 => Ok(OpCode::COMPOUND_MUL),
            0x09 => Ok(OpCode::COMPOUND_DIV),
            0x0A => Ok(OpCode::COMPOUND_MOD),
            0x0B => Ok(OpCode::COMPOUND_EXP),
            0x10 => Ok(OpCode::EQ),
            0x11 => Ok(OpCode::NEQ),
            0x12 => Ok(OpCode::LT),
            0x13 => Ok(OpCode::GT),
            0x14 => Ok(OpCode::LTE),
            0x15 => Ok(OpCode::GTE),
            0x16 => Ok(OpCode::IN),
            0x17 => Ok(OpCode::NOT),
            0x18 => Ok(OpCode::AND),
            0x19 => Ok(OpCode::OR),
            0x1A => Ok(OpCode::STRICT_EQ),
            0x1C => Ok(OpCode::STRICT_NEQ),
            0x1D => Ok(OpCode::UNARY_PLUS),
            0x20 => Ok(OpCode::JMP),
            0x21 => Ok(OpCode::JMP_IF_FALSE),
            0x22 => Ok(OpCode::JMP_IF_TRUE),
            0x23 => Ok(OpCode::FOR_OF_INIT),
            0x24 => Ok(OpCode::FOR_OF_NEXT),
            0x25 => Ok(OpCode::INC_PRE),
            0x26 => Ok(OpCode::INC_POST),
            0x27 => Ok(OpCode::DEC_PRE),
            0x28 => Ok(OpCode::DEC_POST),
            0x29 => Ok(OpCode::FOR_IN_INIT),
            0x2A => Ok(OpCode::FOR_IN_NEXT),
            0x2B => Ok(OpCode::FOR_IN_DONE),
            0x2C => Ok(OpCode::SWITCH_TABLE),
            0x2D => Ok(OpCode::FOR_IN_CLEANUP),
            0x2E => Ok(OpCode::THROW),
            0x2F => Ok(OpCode::TRY_BEGIN),
            0x30 => Ok(OpCode::LOAD_VAR),
            0x31 => Ok(OpCode::STORE_VAR),
            0x32 => Ok(OpCode::LOAD_CONST),
            0x33 => Ok(OpCode::TRY_END),
            0x34 => Ok(OpCode::TRY_FINALLY_BEGIN),
            0x35 => Ok(OpCode::TRY_FINALLY_END),
            0x36 => Ok(OpCode::FOR_OF_DONE),
            0x37 => Ok(OpCode::FOR_OF_CLOSE),
            0x38 => Ok(OpCode::TEMPLATE_STR),
            0x39 => Ok(OpCode::DELETE_PROP_STATIC),
            0x3A => Ok(OpCode::DELETE_PROP_DYNAMIC),
            0x3B => Ok(OpCode::INSTANCEOF),
            0x3E => Ok(OpCode::REST_OBJECT),
            0x40 => Ok(OpCode::CALL),
            0x41 => Ok(OpCode::RETURN),
            0x42 => Ok(OpCode::CALL_NATIVE),
            0x43 => Ok(OpCode::NEW_EXPRESSION),
            0x44 => Ok(OpCode::SUPER_CALL),
            0x45 => Ok(OpCode::SUPER_GET_PROP),
            0x46 => Ok(OpCode::SUPER_STATIC_GET_PROP),
            0x47 => Ok(OpCode::SET_HOME_OBJECT),
            0x48 => Ok(OpCode::DEFINE_ACCESSOR),
            0x50 => Ok(OpCode::IC_GET_PROP),
            0x51 => Ok(OpCode::IC_SET_PROP),
            0x52 => Ok(OpCode::GET_PROP),
            0x53 => Ok(OpCode::GET_PROP_DYNAMIC),
            0x54 => Ok(OpCode::SET_PROP),
            0x55 => Ok(OpCode::SET_PROP_DYNAMIC),
            0x56 => Ok(OpCode::NEW_OBJECT),
            0x57 => Ok(OpCode::NEW_ARRAY),
            0x58 => Ok(OpCode::SET_ELEM),
            0x59 => Ok(OpCode::MEMBER_INC),
            0x5A => Ok(OpCode::MEMBER_DEC),
            0x5B => Ok(OpCode::DYN_MEMBER_INC),
            0x5C => Ok(OpCode::DYN_MEMBER_DEC),
            0x5D => Ok(OpCode::COMPOUND_MEMBER_ADD),
            0x5E => Ok(OpCode::COMPOUND_MEMBER_SUB),
            0x5F => Ok(OpCode::COMPOUND_MEMBER_MUL),
            0x60 => Ok(OpCode::COMPOUND_MEMBER_DIV),
            0x61 => Ok(OpCode::COMPOUND_MEMBER_MOD),
            0x62 => Ok(OpCode::COMPOUND_MEMBER_EXP),
            0x63 => Ok(OpCode::PROFILE_TYPE),
            0x64 => Ok(OpCode::PROFILE_SHAPE),
            0x65 => Ok(OpCode::PROFILE_BRANCH),
            0x66 => Ok(OpCode::PROFILE_CALL),
            0x70 => Ok(OpCode::FORK),
            0x71 => Ok(OpCode::JOIN),
            0x80 => Ok(OpCode::BIT_AND),
            0x81 => Ok(OpCode::BIT_OR),
            0x82 => Ok(OpCode::BIT_XOR),
            0x83 => Ok(OpCode::SHL),
            0x84 => Ok(OpCode::SHR),
            0x85 => Ok(OpCode::USHR),
            0x86 => Ok(OpCode::BIT_NOT),
            0x88 => Ok(OpCode::COMPOUND_AND),
            0x89 => Ok(OpCode::COMPOUND_OR),
            0x8A => Ok(OpCode::COMPOUND_XOR),
            0x8B => Ok(OpCode::COMPOUND_SHL),
            0x8C => Ok(OpCode::COMPOUND_SHR),
            0x8D => Ok(OpCode::COMPOUND_USHR),
            0xF0 => Ok(OpCode::NOP),
            0xF1 => Ok(OpCode::HALT),
            0xF2 => Ok(OpCode::TYPEOF),
            0xF3 => Ok(OpCode::VOID),
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
            OpCode::COMPOUND_ADD => "COMPOUND_ADD",
            OpCode::COMPOUND_SUB => "COMPOUND_SUB",
            OpCode::COMPOUND_MUL => "COMPOUND_MUL",
            OpCode::COMPOUND_DIV => "COMPOUND_DIV",
            OpCode::COMPOUND_MOD => "COMPOUND_MOD",
            OpCode::COMPOUND_EXP => "COMPOUND_EXP",
            OpCode::EQ => "EQ",
            OpCode::NEQ => "NEQ",
            OpCode::LT => "LT",
            OpCode::GT => "GT",
            OpCode::LTE => "LTE",
            OpCode::GTE => "GTE",
            OpCode::IN => "IN",
            OpCode::NOT => "NOT",
            OpCode::AND => "AND",
            OpCode::OR => "OR",
            OpCode::STRICT_EQ => "STRICT_EQ",
            OpCode::STRICT_NEQ => "STRICT_NEQ",
            OpCode::UNARY_PLUS => "UNARY_PLUS",
            OpCode::JMP => "JMP",
            OpCode::JMP_IF_FALSE => "JMP_IF_FALSE",
            OpCode::JMP_IF_TRUE => "JMP_IF_TRUE",
            OpCode::FOR_OF_INIT => "FOR_OF_INIT",
            OpCode::FOR_OF_NEXT => "FOR_OF_NEXT",
            OpCode::INC_PRE => "INC_PRE",
            OpCode::INC_POST => "INC_POST",
            OpCode::DEC_PRE => "DEC_PRE",
            OpCode::DEC_POST => "DEC_POST",
            OpCode::FOR_IN_INIT => "FOR_IN_INIT",
            OpCode::FOR_IN_NEXT => "FOR_IN_NEXT",
            OpCode::FOR_IN_DONE => "FOR_IN_DONE",
            OpCode::SWITCH_TABLE => "SWITCH_TABLE",
            OpCode::FOR_IN_CLEANUP => "FOR_IN_CLEANUP",
            OpCode::THROW => "THROW",
            OpCode::TRY_BEGIN => "TRY_BEGIN",
            OpCode::TRY_END => "TRY_END",
            OpCode::TRY_FINALLY_BEGIN => "TRY_FINALLY_BEGIN",
            OpCode::TRY_FINALLY_END => "TRY_FINALLY_END",
            OpCode::FOR_OF_DONE => "FOR_OF_DONE",
            OpCode::FOR_OF_CLOSE => "FOR_OF_CLOSE",
            OpCode::TEMPLATE_STR => "TEMPLATE_STR",
            OpCode::DELETE_PROP_STATIC => "DELETE_PROP_STATIC",
            OpCode::DELETE_PROP_DYNAMIC => "DELETE_PROP_DYNAMIC",
            OpCode::INSTANCEOF => "INSTANCEOF",
            OpCode::REST_OBJECT => "REST_OBJECT",
            OpCode::LOAD_VAR => "LOAD_VAR",
            OpCode::STORE_VAR => "STORE_VAR",
            OpCode::LOAD_CONST => "LOAD_CONST",
            OpCode::CALL => "CALL",
            OpCode::RETURN => "RETURN",
            OpCode::CALL_NATIVE => "CALL_NATIVE",
            OpCode::NEW_EXPRESSION => "NEW_EXPRESSION",
            OpCode::SUPER_CALL => "SUPER_CALL",
            OpCode::SUPER_GET_PROP => "SUPER_GET_PROP",
            OpCode::SUPER_STATIC_GET_PROP => "SUPER_STATIC_GET_PROP",
            OpCode::SET_HOME_OBJECT => "SET_HOME_OBJECT",
            OpCode::DEFINE_ACCESSOR => "DEFINE_ACCESSOR",
            OpCode::IC_GET_PROP => "IC_GET_PROP",
            OpCode::IC_SET_PROP => "IC_SET_PROP",
            OpCode::GET_PROP => "GET_PROP",
            OpCode::GET_PROP_DYNAMIC => "GET_PROP_DYNAMIC",
            OpCode::SET_PROP => "SET_PROP",
            OpCode::SET_PROP_DYNAMIC => "SET_PROP_DYNAMIC",
            OpCode::NEW_OBJECT => "NEW_OBJECT",
            OpCode::NEW_ARRAY => "NEW_ARRAY",
            OpCode::SET_ELEM => "SET_ELEM",
            OpCode::MEMBER_INC => "MEMBER_INC",
            OpCode::MEMBER_DEC => "MEMBER_DEC",
            OpCode::DYN_MEMBER_INC => "DYN_MEMBER_INC",
            OpCode::DYN_MEMBER_DEC => "DYN_MEMBER_DEC",
            OpCode::COMPOUND_MEMBER_ADD => "COMPOUND_MEMBER_ADD",
            OpCode::COMPOUND_MEMBER_SUB => "COMPOUND_MEMBER_SUB",
            OpCode::COMPOUND_MEMBER_MUL => "COMPOUND_MEMBER_MUL",
            OpCode::COMPOUND_MEMBER_DIV => "COMPOUND_MEMBER_DIV",
            OpCode::COMPOUND_MEMBER_MOD => "COMPOUND_MEMBER_MOD",
            OpCode::COMPOUND_MEMBER_EXP => "COMPOUND_MEMBER_EXP",
            OpCode::PROFILE_TYPE => "PROFILE_TYPE",
            OpCode::PROFILE_SHAPE => "PROFILE_SHAPE",
            OpCode::PROFILE_BRANCH => "PROFILE_BRANCH",
            OpCode::PROFILE_CALL => "PROFILE_CALL",
            OpCode::FORK => "FORK",
            OpCode::JOIN => "JOIN",
            OpCode::BIT_AND => "BIT_AND",
            OpCode::BIT_OR => "BIT_OR",
            OpCode::BIT_XOR => "BIT_XOR",
            OpCode::SHL => "SHL",
            OpCode::SHR => "SHR",
            OpCode::USHR => "USHR",
            OpCode::BIT_NOT => "BIT_NOT",
            OpCode::COMPOUND_AND => "COMPOUND_AND",
            OpCode::COMPOUND_OR => "COMPOUND_OR",
            OpCode::COMPOUND_XOR => "COMPOUND_XOR",
            OpCode::COMPOUND_SHL => "COMPOUND_SHL",
            OpCode::COMPOUND_SHR => "COMPOUND_SHR",
            OpCode::COMPOUND_USHR => "COMPOUND_USHR",
            OpCode::NOP => "NOP",
            OpCode::HALT => "HALT",
            OpCode::TYPEOF => "TYPEOF",
            OpCode::VOID => "VOID",
        };
        write!(f, "{name}")
    }
}

/// 4-byte instruction.
///
/// Layout: `[opcode: u8] [rd: u8] [a: u8] [b: u8]`
/// - `rd` — destination register
/// - `a` — first source register, or imm16 low byte
/// - `b` — second source register, or imm16 high byte
pub type Instr = u32;

pub fn encode(op: OpCode, rd: u8, a: u8, b: u8) -> Instr {
    ((b as Instr) << 24) | ((a as Instr) << 16) | ((rd as Instr) << 8) | (op as Instr)
}

pub fn opcode(instr: Instr) -> OpCode {
    OpCode::try_from((instr & 0xFF) as u8).unwrap_or(OpCode::NOP)
}

pub fn rd(instr: Instr) -> u8 {
    ((instr >> 8) & 0xFF) as u8
}

pub fn a(instr: Instr) -> u8 {
    ((instr >> 16) & 0xFF) as u8
}

pub fn b(instr: Instr) -> u8 {
    ((instr >> 24) & 0xFF) as u8
}

pub fn imm16(instr: Instr) -> u16 {
    ((instr >> 16) & 0xFFFF) as u16
}

pub fn offset16(instr: Instr) -> i16 {
    ((instr >> 16) & 0xFFFF) as i16
}

pub fn encode_jmp(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP, 0, lo, hi)
}

pub fn encode_jmp_if_false(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_FALSE, rd, lo, hi)
}

pub fn encode_jmp_if_true(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_TRUE, rd, lo, hi)
}

pub fn encode_try_begin(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::TRY_BEGIN, 0, lo, hi)
}

pub fn encode_try_finally_begin(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::TRY_FINALLY_BEGIN, 0, lo, hi)
}
