//! Saturating integer conversions for counts and indices stored in schema columns.

/// `usize` to `i64`, saturating (database ids and counts).
pub fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `usize` to `u32`, saturating (line numbers and small counts).
pub fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
