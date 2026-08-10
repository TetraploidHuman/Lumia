//! Semantic token legend, absolute spans, and LSP delta encoding.

use lumia_syntax::{byte_to_line_col, line_starts};

/// Legend token types (index = `token_type`). Keep aligned with common editor themes.
pub const TOKEN_TYPES: &[&str] = &[
    "function",   // 0
    "method",     // 1
    "variable",   // 2
    "parameter",  // 3
    "struct",     // 4
    "enum",       // 5
    "enumMember", // 6
    "property",   // 7
    "type",       // 8
    "keyword",    // 9
];

pub const TOKEN_MODIFIERS: &[&str] = &["declaration", "readonly", "defaultLibrary"];

pub(super) const MOD_DECL: u32 = 1 << 0;
pub(super) const MOD_READONLY: u32 = 1 << 1;
pub(super) const MOD_DEFAULT_LIB: u32 = 1 << 2;

pub(super) const TY_FUNCTION: u32 = 0;
pub(super) const TY_METHOD: u32 = 1;
pub(super) const TY_VARIABLE: u32 = 2;
pub(super) const TY_PARAMETER: u32 = 3;
pub(super) const TY_STRUCT: u32 = 4;
pub(super) const TY_ENUM: u32 = 5;
pub(super) const TY_ENUM_MEMBER: u32 = 6;
pub(super) const TY_PROPERTY: u32 = 7;
pub(super) const TY_TYPE: u32 = 8;
pub(super) const TY_KEYWORD: u32 = 9;

#[derive(Clone, Copy)]
pub(super) struct AbsToken {
    pub start: usize,
    pub end: usize,
    pub ty: u32,
    pub mods: u32,
}

pub(super) fn encode_deltas(src: &str, abs: &[AbsToken]) -> Vec<u32> {
    let starts = line_starts(src);
    let mut out = Vec::with_capacity(abs.len() * 5);
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for t in abs {
        if t.end <= t.start || t.end > src.len() {
            continue;
        }
        let (line, col) = byte_to_line_col(&starts, lumia_syntax::BytePos(t.start as u32));
        let line = line.saturating_sub(1);
        let col = col.saturating_sub(1);
        let length = (t.end - t.start) as u32;
        let delta_line = line.saturating_sub(prev_line);
        let delta_start = if delta_line == 0 {
            col.saturating_sub(prev_start)
        } else {
            col
        };
        out.push(delta_line);
        out.push(delta_start);
        out.push(length);
        out.push(t.ty);
        out.push(t.mods);
        prev_line = line;
        prev_start = col;
    }
    out
}

pub(super) fn push(out: &mut Vec<AbsToken>, start: usize, end: usize, ty: u32, mods: u32) {
    if end > start {
        out.push(AbsToken {
            start,
            end,
            ty,
            mods,
        });
    }
}
