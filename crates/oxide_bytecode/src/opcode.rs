use std::fmt;

/// Generate the `OpCode` enum plus its `TryFrom<u8>` and `Display` impls from a
/// single table. Each row is `VARIANT = 0xXX => "DISPLAY"` — the byte value is
/// the enum discriminant, the `TryFrom` match arm, and the `Display` name come
/// from the same row, so the three tables can never drift. A new opcode is one
/// row.
///
/// Grouping comments (`// -- Arithmetic --`) are stripped by the lexer before
/// macro expansion, so they can be interleaved freely between rows.
macro_rules! define_opcodes {
    ( $( $name:ident = $val:literal => $disp:literal ),+ $(,)? ) => {
        /// OpCode for register-based bytecode VM.
        ///
        /// Organized in groups of 16 for readability. Implemented opcodes have
        /// emitter support in the compiler; placeholder opcodes are reserved
        /// for future phases (IC, profiling, parallelization).
        #[repr(u8)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[allow(non_camel_case_types)]
        pub enum OpCode {
            $( $name = $val, )+
        }

        impl TryFrom<u8> for OpCode {
            type Error = ();

            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $( $val => Ok(OpCode::$name), )+
                    _ => Err(()),
                }
            }
        }

        impl fmt::Display for OpCode {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let name = match self {
                    $( OpCode::$name => $disp, )+
                };
                write!(f, "{name}")
            }
        }
    };
}

define_opcodes! {
    // ── Arithmetic (0x00-0x0F) ──
    ADD = 0x00 => "ADD",
    SUB = 0x01 => "SUB",
    MUL = 0x02 => "MUL",
    DIV = 0x03 => "DIV",
    MOD = 0x04 => "MOD",
    NEG = 0x05 => "NEG",
    COMPOUND_ADD = 0x06 => "COMPOUND_ADD",
    COMPOUND_SUB = 0x07 => "COMPOUND_SUB",
    COMPOUND_MUL = 0x08 => "COMPOUND_MUL",
    COMPOUND_DIV = 0x09 => "COMPOUND_DIV",
    COMPOUND_MOD = 0x0A => "COMPOUND_MOD",
    COMPOUND_EXP = 0x0B => "COMPOUND_EXP",

    // -- Comparison (0x10-0x1F) --
    EQ = 0x10 => "EQ",
    NEQ = 0x11 => "NEQ",
    LT = 0x12 => "LT",
    GT = 0x13 => "GT",
    LTE = 0x14 => "LTE",
    GTE = 0x15 => "GTE",
    IN = 0x16 => "IN",
    NOT = 0x17 => "NOT",
    AND = 0x18 => "AND",
    OR = 0x19 => "OR",
    STRICT_EQ = 0x1A => "STRICT_EQ",
    STRICT_NEQ = 0x1C => "STRICT_NEQ",
    UNARY_PLUS = 0x1D => "UNARY_PLUS",

    // -- Control Flow (0x20-0x2F) --
    JMP = 0x20 => "JMP",
    JMP_IF_FALSE = 0x21 => "JMP_IF_FALSE",
    JMP_IF_TRUE = 0x22 => "JMP_IF_TRUE",
    FOR_OF_INIT = 0x23 => "FOR_OF_INIT",
    FOR_OF_NEXT = 0x24 => "FOR_OF_NEXT",

    // -- Update (0x25-0x28) --
    INC_PRE = 0x25 => "INC_PRE",
    INC_POST = 0x26 => "INC_POST",
    DEC_PRE = 0x27 => "DEC_PRE",
    DEC_POST = 0x28 => "DEC_POST",

    FOR_IN_INIT = 0x29 => "FOR_IN_INIT",
    FOR_IN_NEXT = 0x2A => "FOR_IN_NEXT",
    FOR_IN_DONE = 0x2B => "FOR_IN_DONE",
    SWITCH_TABLE = 0x2C => "SWITCH_TABLE",
    FOR_IN_CLEANUP = 0x2D => "FOR_IN_CLEANUP",

    // -- Exception (0x2E-0x2F, 0x33-0x35) --
    THROW = 0x2E => "THROW",
    TRY_BEGIN = 0x2F => "TRY_BEGIN",
    TRY_END = 0x33 => "TRY_END",
    TRY_FINALLY_BEGIN = 0x34 => "TRY_FINALLY_BEGIN",
    TRY_FINALLY_END = 0x35 => "TRY_FINALLY_END",
    FOR_OF_DONE = 0x36 => "FOR_OF_DONE",
    FOR_OF_CLOSE = 0x37 => "FOR_OF_CLOSE",

    // -- Template Literal (0x38) --
    TEMPLATE_STR = 0x38 => "TEMPLATE_STR",

    // -- Small Language Features (0x39-0x3B) --
    DELETE_PROP_STATIC = 0x39 => "DELETE_PROP_STATIC",
    DELETE_PROP_DYNAMIC = 0x3A => "DELETE_PROP_DYNAMIC",
    INSTANCEOF = 0x3B => "INSTANCEOF",
    REST_OBJECT = 0x3E => "REST_OBJECT",

    // -- Variable (0x30-0x32) --
    LOAD_VAR = 0x30 => "LOAD_VAR",
    STORE_VAR = 0x31 => "STORE_VAR",
    LOAD_CONST = 0x32 => "LOAD_CONST",

    // -- Call (0x40-0x4F) --
    CALL = 0x40 => "CALL",
    RETURN = 0x41 => "RETURN",
    CALL_NATIVE = 0x42 => "CALL_NATIVE",
    NEW_EXPRESSION = 0x43 => "NEW_EXPRESSION",
    SUPER_CALL = 0x44 => "SUPER_CALL",
    SUPER_GET_PROP = 0x45 => "SUPER_GET_PROP",
    SUPER_STATIC_GET_PROP = 0x46 => "SUPER_STATIC_GET_PROP",
    SET_HOME_OBJECT = 0x47 => "SET_HOME_OBJECT",
    DEFINE_ACCESSOR = 0x48 => "DEFINE_ACCESSOR",
    CREATE_CLOSURE = 0x4A => "CREATE_CLOSURE",
    CREATE_REGEXP = 0x4B => "CREATE_REGEXP",

    // ── Object Property (0x50-0x5F) ──
    IC_GET_PROP = 0x50 => "IC_GET_PROP",
    IC_SET_PROP = 0x51 => "IC_SET_PROP",
    GET_PROP = 0x52 => "GET_PROP",
    GET_PROP_DYNAMIC = 0x53 => "GET_PROP_DYNAMIC",
    SET_PROP = 0x54 => "SET_PROP",
    SET_PROP_DYNAMIC = 0x55 => "SET_PROP_DYNAMIC",
    NEW_OBJECT = 0x56 => "NEW_OBJECT",
    NEW_ARRAY = 0x57 => "NEW_ARRAY",
    SET_ELEM = 0x58 => "SET_ELEM",

    // -- Member Update (0x59-0x62) --
    MEMBER_INC = 0x59 => "MEMBER_INC",
    MEMBER_DEC = 0x5A => "MEMBER_DEC",
    DYN_MEMBER_INC = 0x5B => "DYN_MEMBER_INC",
    DYN_MEMBER_DEC = 0x5C => "DYN_MEMBER_DEC",
    COMPOUND_MEMBER_ADD = 0x5D => "COMPOUND_MEMBER_ADD",
    COMPOUND_MEMBER_SUB = 0x5E => "COMPOUND_MEMBER_SUB",
    COMPOUND_MEMBER_MUL = 0x5F => "COMPOUND_MEMBER_MUL",
    COMPOUND_MEMBER_DIV = 0x60 => "COMPOUND_MEMBER_DIV",
    COMPOUND_MEMBER_MOD = 0x61 => "COMPOUND_MEMBER_MOD",
    COMPOUND_MEMBER_EXP = 0x62 => "COMPOUND_MEMBER_EXP",

    // -- Profiling - placeholders (0x63-0x6F) --
    PROFILE_TYPE = 0x63 => "PROFILE_TYPE",
    PROFILE_SHAPE = 0x64 => "PROFILE_SHAPE",
    PROFILE_BRANCH = 0x65 => "PROFILE_BRANCH",
    PROFILE_CALL = 0x66 => "PROFILE_CALL",

    // ── Parallel — placeholders (0x70-0x7F) ──
    FORK = 0x70 => "FORK",
    JOIN = 0x71 => "JOIN",
    GET_PRIVATE = 0x72 => "GET_PRIVATE",
    SET_PRIVATE = 0x73 => "SET_PRIVATE",
    INIT_PRIVATE = 0x74 => "INIT_PRIVATE",
    PRIVATE_BRAND_IN = 0x75 => "PRIVATE_BRAND_IN",

    // -- Bitwise (0x80-0x8F) --
    BIT_AND = 0x80 => "BIT_AND",
    BIT_OR = 0x81 => "BIT_OR",
    BIT_XOR = 0x82 => "BIT_XOR",
    SHL = 0x83 => "SHL",
    SHR = 0x84 => "SHR",
    USHR = 0x85 => "USHR",
    BIT_NOT = 0x86 => "BIT_NOT",
    COMPOUND_AND = 0x88 => "COMPOUND_AND",
    COMPOUND_OR = 0x89 => "COMPOUND_OR",
    COMPOUND_XOR = 0x8A => "COMPOUND_XOR",
    COMPOUND_SHL = 0x8B => "COMPOUND_SHL",
    COMPOUND_SHR = 0x8C => "COMPOUND_SHR",
    COMPOUND_USHR = 0x8D => "COMPOUND_USHR",
    NULLISH = 0x8E => "NULLISH",
    JMP_IF_NULLISH = 0x8F => "JMP_IF_NULLISH",

    // ── Misc (0xF0-0xFF) ──
    NOP = 0xF0 => "NOP",
    HALT = 0xF1 => "HALT",
    TYPEOF = 0xF2 => "TYPEOF",
    VOID = 0xF3 => "VOID",
}

impl OpCode {
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

pub fn encode_jmp_if_nullish(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_NULLISH, rd, lo, hi)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_table_is_byte_identical() {
        for byte in 0u8..=u8::MAX {
            if let Ok(op) = OpCode::try_from(byte) {
                assert_eq!(op as u8, byte, "discriminant mismatch for 0x{byte:02X}");
                assert!(!op.to_string().is_empty(), "empty Display for 0x{byte:02X}");
            }
        }

        assert_eq!(OpCode::ADD as u8, 0x00);
        assert_eq!(OpCode::STRICT_EQ as u8, 0x1A);
        assert_eq!(OpCode::JMP_IF_NULLISH as u8, 0x8F);
        assert_eq!(OpCode::VOID as u8, 0xF3);
        assert_eq!(OpCode::ADD.to_string(), "ADD");
        assert_eq!(OpCode::COMPOUND_MEMBER_EXP.to_string(), "COMPOUND_MEMBER_EXP");

        assert!(OpCode::try_from(0x1B).is_err());
        assert!(OpCode::try_from(0xFF).is_err());

        let ic_count = (0u8..=u8::MAX)
            .filter_map(|b| OpCode::try_from(b).ok())
            .filter(OpCode::has_ic_ext_words)
            .count();
        assert_eq!(ic_count, 10);
    }
}
