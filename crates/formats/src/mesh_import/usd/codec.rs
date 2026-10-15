//! Pieces of the USD Crate (`.usdc`) binary format shared by the reader and the writer: value
//! type ids, the LZ4 framing (one chunk-count byte, then LZ4 blocks) and the integer coding
//! (most common delta + 2-bit width codes + variable-width deltas).

use super::KIND;
use crate::{FormatError, Result, malformed};

pub(crate) const MAGIC: &[u8; 8] = b"PXR-USDC";
/// Size of the bootstrap header: magic, version, TOC offset, reserved.
pub(crate) const BOOTSTRAP_LEN: usize = 88;
/// Arrays shorter than this are never stored compressed.
pub(crate) const MIN_COMPRESSED_ARRAY: usize = 16;

/// ValueRep bits.
pub(crate) const IS_ARRAY: u64 = 1 << 63;
pub(crate) const IS_INLINED: u64 = 1 << 62;
pub(crate) const IS_COMPRESSED: u64 = 1 << 61;
pub(crate) const IS_ARRAY_EDIT: u64 = 1 << 60;
pub(crate) const PAYLOAD_MASK: u64 = (1 << 48) - 1;

/// Value type ids (crate data types).
pub(crate) mod ty {
    pub const BOOL: u8 = 1;
    pub const UCHAR: u8 = 2;
    pub const INT: u8 = 3;
    pub const UINT: u8 = 4;
    pub const INT64: u8 = 5;
    pub const UINT64: u8 = 6;
    pub const HALF: u8 = 7;
    pub const FLOAT: u8 = 8;
    pub const DOUBLE: u8 = 9;
    pub const STRING: u8 = 10;
    pub const TOKEN: u8 = 11;
    pub const ASSET_PATH: u8 = 12;
    pub const MATRIX2D: u8 = 13;
    pub const MATRIX4D: u8 = 15;
    pub const QUATD: u8 = 16;
    pub const QUATH: u8 = 18;
    pub const VEC2D: u8 = 19;
    pub const VEC4I: u8 = 30;
    pub const TOKEN_LIST_OP: u8 = 32;
    pub const PATH_LIST_OP: u8 = 34;
    pub const REFERENCE_LIST_OP: u8 = 35;
    pub const PATH_VECTOR: u8 = 40;
    pub const TOKEN_VECTOR: u8 = 41;
    pub const SPECIFIER: u8 = 42;
    pub const PERMISSION: u8 = 43;
    pub const VARIABILITY: u8 = 44;
    pub const TIME_SAMPLES: u8 = 46;
    pub const DOUBLE_VECTOR: u8 = 48;
}

/// Scalar kind and component count of the numeric types (vectors, quaternions, matrices).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Scalar {
    U8,
    I32,
    U32,
    I64,
    U64,
    F16,
    F32,
    F64,
}

impl Scalar {
    pub fn size(self) -> usize {
        match self {
            Scalar::U8 => 1,
            Scalar::F16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::I64 | Scalar::U64 | Scalar::F64 => 8,
        }
    }
}

/// (scalar, components) of numeric type `t`, or None for non-numeric types.
pub(crate) fn numeric(t: u8) -> Option<(Scalar, usize)> {
    use Scalar::*;
    Some(match t {
        ty::BOOL | ty::UCHAR => (U8, 1),
        ty::INT => (I32, 1),
        ty::UINT => (U32, 1),
        ty::INT64 => (I64, 1),
        ty::UINT64 => (U64, 1),
        ty::HALF => (F16, 1),
        ty::FLOAT => (F32, 1),
        ty::DOUBLE => (F64, 1),
        ty::MATRIX2D => (F64, 4),
        14 => (F64, 9),
        ty::MATRIX4D => (F64, 16),
        ty::QUATD => (F64, 4),
        17 => (F32, 4),
        ty::QUATH => (F16, 4),
        // Vec{2,3,4}{d,f,h,i}: ids 19..=30 in that order.
        ty::VEC2D..=ty::VEC4I => {
            let k = t - ty::VEC2D;
            let scalar = [F64, F32, F16, I32][usize::from(k % 4)];
            (scalar, usize::from(k / 4) + 2)
        }
        _ => return None,
    })
}

/// Decompress Crate's LZ4 framing into at most `max_out` bytes.
pub(crate) fn decompress(input: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let bad = |e: lz4_flex::block::DecompressError| malformed(KIND, format!("LZ4 data: {e}"));
    let (&chunks, mut rest) = input.split_first().ok_or_else(|| malformed(KIND, "empty compressed block"))?;
    // LZ4 expands at most ~255×: never allocate more than the input can produce.
    let mut out = vec![0u8; max_out.min(input.len().saturating_mul(256).saturating_add(64))];
    let written = if chunks == 0 {
        lz4_flex::block::decompress_into(rest, &mut out).map_err(bad)?
    } else {
        if chunks > 127 {
            return Err(malformed(KIND, "bad LZ4 chunk count"));
        }
        let mut total = 0usize;
        for _ in 0..chunks {
            let size = rest.get(..4).and_then(|b| b.try_into().ok()).map(i32::from_le_bytes).ok_or_else(|| malformed(KIND, "truncated LZ4 chunk"))?;
            let size = usize::try_from(size).map_err(|_| malformed(KIND, "negative LZ4 chunk size"))?;
            let chunk = rest.get(4..4usize.saturating_add(size)).ok_or_else(|| malformed(KIND, "truncated LZ4 chunk"))?;
            rest = rest.get(4 + size..).unwrap_or_default();
            total += lz4_flex::block::decompress_into(chunk, out.get_mut(total..).unwrap_or_default()).map_err(bad)?;
        }
        total
    };
    out.truncate(written);
    Ok(out)
}

/// Crate's LZ4 framing for one chunk (inputs are far below LZ4's ~2 GB chunk limit).
pub(crate) fn compress(input: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8];
    out.extend_from_slice(&lz4_flex::block::compress(input));
    out
}

/// Upper bound of the encoded size of `n` integers of `width` bytes.
pub(crate) fn encoded_ints_len(n: usize, width: usize) -> usize {
    if n == 0 { 0 } else { width.saturating_add(n.saturating_mul(2).saturating_add(7) / 8).saturating_add(n.saturating_mul(width)) }
}

fn read_signed(b: &[u8], at: usize, width: usize) -> Option<i64> {
    let s = b.get(at..at.checked_add(width)?)?;
    Some(match width {
        1 => i64::from(i8::from_le_bytes(s.try_into().ok()?)),
        2 => i64::from(i16::from_le_bytes(s.try_into().ok()?)),
        4 => i64::from(i32::from_le_bytes(s.try_into().ok()?)),
        _ => i64::from_le_bytes(s.try_into().ok()?),
    })
}

/// Decode `n` integers of `width` (4 or 8) bytes; 32-bit values are sign-extended (reinterpret
/// as u32 for unsigned data).
pub(crate) fn decode_ints(data: &[u8], n: usize, width: usize) -> Result<Vec<i64>> {
    let err = || malformed(KIND, "truncated integer array");
    if n == 0 {
        return Ok(Vec::new());
    }
    let common = read_signed(data, 0, width).ok_or_else(err)?;
    let codes_at = width;
    let mut at = codes_at + n.div_ceil(4);
    let mut out = Vec::with_capacity(n);
    let mut prev = 0i64;
    for i in 0..n {
        let code = (data.get(codes_at + i / 4).ok_or_else(err)? >> (2 * (i % 4))) & 3;
        let delta = match code {
            0 => common,
            c => {
                let w = (width >> (3 - c as usize)).max(1);
                let v = read_signed(data, at, w).ok_or_else(err)?;
                at += w;
                v
            }
        };
        prev = if width == 4 { i64::from((prev as i32).wrapping_add(delta as i32)) } else { prev.wrapping_add(delta) };
        out.push(prev);
    }
    Ok(out)
}

/// Encode integers of `width` (4 or 8) bytes (values must fit the width).
pub(crate) fn encode_ints(values: &[i64], width: usize) -> Vec<u8> {
    if values.is_empty() {
        return Vec::new();
    }
    let wrap = |v: i64| if width == 4 { i64::from(v as i32) } else { v };
    let mut prev = 0i64;
    let deltas: Vec<i64> = values
        .iter()
        .map(|&v| {
            let d = wrap(v.wrapping_sub(prev));
            prev = v;
            d
        })
        .collect();
    let mut counts = std::collections::HashMap::new();
    for d in &deltas {
        *counts.entry(*d).or_insert(0usize) += 1;
    }
    let common = counts.into_iter().max_by_key(|&(v, c)| (c, v)).map_or(0, |(v, _)| v);
    let mut out = Vec::with_capacity(encoded_ints_len(values.len(), width));
    out.extend_from_slice(&common.to_le_bytes()[..width]);
    let codes_at = out.len();
    out.resize(codes_at + values.len().div_ceil(4), 0);
    for (i, &d) in deltas.iter().enumerate() {
        let small = |bits: u32| d >= -(1i64 << (bits - 1)) && d < (1i64 << (bits - 1));
        let code: u8 = if d == common {
            0
        } else if small(width as u32 * 2) {
            1
        } else if small(width as u32 * 4) {
            2
        } else {
            3
        };
        if code > 0 {
            let w = (width >> (3 - code as usize)).max(1);
            out.extend_from_slice(&d.to_le_bytes()[..w]);
        }
        if let Some(b) = out.get_mut(codes_at + i / 4) {
            *b |= code << (2 * (i % 4));
        }
    }
    out
}

/// Error for counts that cannot be real (more items than bytes left could describe).
pub(crate) fn check_count(n: u64, cap: usize) -> Result<usize> {
    usize::try_from(n).ok().filter(|n| *n <= cap).ok_or_else(|| FormatError::TooLarge(format!("USD array of {n} items (limit {cap})")))
}
