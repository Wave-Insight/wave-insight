//! Rendering a decoded value with [`ShowType`].
//!
//! This is the formatter the viewer has always used, moved off the deleted
//! `BitsData` type: a value is first packed into the legacy `(xz, num)` byte
//! layout (two bytes per eight bits, big endian) and then turned into text.
//! Keeping the exact same packing means the output is unchanged, and the test at
//! the bottom is the original test, kept as the oracle.
//!
//! The one deliberate difference: the bit count is clamped to what the value
//! holds. The old `BitsData::to_string` subtracted the display width from the
//! byte width and panicked with "attempt to subtract with overflow" when a dump
//! was narrower than the design width.

use crate::data_struct::ShowType;

/// Render one packed value: `chunk` is the `(xz, num)` byte layout, `bitcount`
/// the number of significant bits.
pub(crate) fn render_chunk(chunk: &[(u8, u8)], bitcount: usize, show: &ShowType) -> String {
    let bitcount = bitcount.min(chunk.len() * 8);
    match show {
        ShowType::Hex => hex_string(chunk, bitcount),
        ShowType::UInt => uint_string(chunk),
        ShowType::SInt => sint_string(chunk, bitcount),
        ShowType::Oct => oct_string(chunk, bitcount),
        ShowType::Bin => bin_string(chunk, bitcount),
        ShowType::Ascii => asc_string(chunk),
    }
}

fn bin_string(value: &[(u8, u8)], bitcount: usize) -> String {
    let drop_head = (value.len() * 8).saturating_sub(bitcount);
    let ret: Vec<u8> = value
        .iter()
        .flat_map(|(xz, num)| {
            (0..8)
                .rev()
                .map(move |idx| match (xz & (1 << idx), num & (1 << idx)) {
                    (0, 0) => b'0',
                    (0, _) => b'1',
                    (_, 0) => b'x',
                    (_, _) => b'z',
                })
        })
        .skip(drop_head)
        .collect();
    String::from_utf8(ret).unwrap_or_else(|_| "error".to_owned())
}

fn oct_string(value: &[(u8, u8)], bitcount: usize) -> String {
    let drop_head = ((value.len() * 8 + 2) / 3).saturating_sub((bitcount + 2) / 3);
    let mut state = (value.len() % 3) as u8;
    let mut pad_head = (0u8, 0u8);
    let ret: Vec<u8> = value
        .iter()
        .flat_map(|(xz, num)| {
            if state == 0 {
                state = 2;
                let ret = vec![
                    (xz >> 5, num >> 5),
                    ((xz >> 2) & 0b00000111, (num >> 2) & 0b00000111),
                ];
                pad_head = (xz & 0b00000011, num & 0b00000011);
                ret
            } else if state == 1 {
                state = 0;
                let ret = vec![
                    (
                        (pad_head.0 << 2) + (xz >> 6),
                        (pad_head.1 << 2) + (num >> 6),
                    ),
                    ((xz >> 3) & 0b00000111, (num >> 3) & 0b00000111),
                    (xz & 0b00000111, num & 0b00000111),
                ];
                pad_head = (0, 0);
                ret
            } else {
                state = 1;
                let ret = vec![
                    ((pad_head.0 << 1) + (xz >> 7), (pad_head.1 << 1) + (num >> 7)),
                    ((xz >> 4) & 0b00000111, (num >> 4) & 0b00000111),
                    ((xz >> 1) & 0b00000111, (num >> 1) & 0b00000111),
                ];
                pad_head = (xz & 0b00000001, num & 0b00000001);
                ret
            }
        })
        .map(|(xz, num)| match (xz, num) {
            (0, _) => b'0' + num,
            _ => {
                if xz & (!num) == 0 {
                    b'z'
                } else {
                    b'x'
                }
            }
        })
        .skip(drop_head)
        .collect();
    String::from_utf8(ret).unwrap_or_else(|_| "error".to_owned())
}

fn hex_string(value: &[(u8, u8)], bitcount: usize) -> String {
    let drop_head = if (value.len() * 8).saturating_sub(bitcount) >= 4 {
        1
    } else {
        0
    };
    let convert = |xz: u8, num: u8| match (xz, num) {
        (0, _) => {
            if num < 10 {
                b'0' + num
            } else {
                b'a' - 10 + num
            }
        }
        (_, _) => {
            if xz & (!num) == 0 {
                b'z'
            } else {
                b'x'
            }
        }
    };
    let ret: Vec<u8> = value
        .iter()
        .flat_map(|(xz, num)| [(xz >> 4, num >> 4), (xz & 0b00001111, num & 0b00001111)])
        .map(|(xz, num)| convert(xz, num))
        .skip(drop_head)
        .collect();
    String::from_utf8(ret).unwrap_or_else(|_| "error".to_owned())
}

fn uint_string(value: &[(u8, u8)]) -> String {
    use num::BigUint;
    let has_xz = value.iter().map(|(xz, _)| xz).any(|&xz| xz != 0);
    if has_xz {
        "X".to_owned()
    } else {
        let num: Vec<u8> = value.iter().map(|(_, num)| *num).collect();
        let value = BigUint::from_bytes_be(&num);
        format!("{value}")
    }
}

fn sint_string(value: &[(u8, u8)], bitcount: usize) -> String {
    use num::{bigint::Sign, bigint::ToBigInt, BigInt, BigUint};
    let has_xz = value.iter().map(|(xz, _)| xz).any(|&xz| xz != 0);
    if has_xz {
        "X".to_owned()
    } else {
        let num: Vec<u8> = value.iter().map(|(_, num)| *num).collect();
        let value = BigUint::from_bytes_be(&num);
        //a one bit or empty value has no sign bit to test
        let bits = bitcount.max(1) as u32;
        let bound = BigUint::new(vec![2]).pow(bits - 1);
        let value_to_sint = if value >= bound {
            value.to_bigint().unwrap() - BigInt::new(Sign::Plus, vec![2]).pow(bits)
        } else {
            value.to_bigint().unwrap()
        };
        format!("{value_to_sint}")
    }
}

fn asc_string(value: &[(u8, u8)]) -> String {
    let vec = value
        .iter()
        .map(|(xz, num)| if *xz == 0 { *num } else { 0 })
        .collect();
    String::from_utf8(vec).unwrap_or_else(|_| "invalid".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(xz, num)` byte pairs, the layout the formatter works on.
    fn chunk(pairs: &[(u8, u8)]) -> Vec<(u8, u8)> {
        pairs.to_vec()
    }

    /// The original `signal_data.rs` test, unchanged: this is the oracle for
    /// "moving the formatter must not change a single character".
    #[test]
    fn oracle() {
        let two = chunk(&[(0, 1), (0, 23)]);
        assert_eq!(bin_string(&two, 10), "0100010111".to_owned());
        assert_eq!(oct_string(&two, 10), "0427".to_owned());
        assert_eq!(hex_string(&two, 10), "117".to_owned());
        assert_eq!(uint_string(&two), "279".to_owned());
        assert_eq!(sint_string(&two, 10), "279".to_owned());

        let b = chunk(&[(0, 1), (0, 123)]);
        assert_eq!(bin_string(&b, 10), "0101111011".to_owned());
        assert_eq!(oct_string(&b, 10), "0573".to_owned());
        assert_eq!(hex_string(&b, 10), "17b".to_owned());
        assert_eq!(sint_string(&b, 10), "379".to_owned());

        let z = chunk(&[(1, 1), (0, 123)]);
        assert_eq!(bin_string(&z, 10), "0z01111011".to_owned());
        assert_eq!(oct_string(&z, 10), "0z73".to_owned());
        assert_eq!(hex_string(&z, 10), "z7b".to_owned());
        assert_eq!(uint_string(&z), "X".to_owned());
        assert_eq!(sint_string(&z, 10), "X".to_owned());

        let x = chunk(&[(1, 32), (0, 123)]);
        assert_eq!(bin_string(&x, 14), "10000x01111011".to_owned());
        assert_eq!(oct_string(&x, 14), "20x73".to_owned());
        assert_eq!(hex_string(&x, 14), "2x7b".to_owned());

        let mixed = chunk(&[(1, 2), (6, 123)]);
        assert_eq!(bin_string(&mixed, 15), "000001x01111xz1".to_owned());
        assert_eq!(oct_string(&mixed, 15), "01x7x".to_owned());
        assert_eq!(hex_string(&mixed, 15), "0x7x".to_owned());

        let signed = chunk(&[(0, 2), (0, 23)]);
        assert_eq!(sint_string(&signed, 10), "-489".to_owned());
    }

    #[test]
    fn the_bit_count_is_clamped_instead_of_panicking() {
        //a dump narrower than the design used to underflow in hex/bin
        let narrow = chunk(&[(0, 0xff)]);
        assert_eq!(hex_string(&narrow, 64), "ff");
        assert_eq!(bin_string(&narrow, 64), "11111111");
        assert_eq!(oct_string(&narrow, 64), "377");
        //and an empty value still renders something
        assert_eq!(bin_string(&[], 0), "");
        assert_eq!(hex_string(&[], 0), "");
        assert_eq!(sint_string(&[], 8), "0");
    }

    #[test]
    fn ascii_and_signed_edge_cases() {
        assert_eq!(asc_string(&[(0, b'H'), (0, b'i')]), "Hi");
        //an unknown byte renders as a NUL instead of failing
        assert_eq!(asc_string(&[(1, b'H')]), "\0");
        //one bit signed values: 1 is -1, 0 is 0
        assert_eq!(sint_string(&[(0, 1)], 1), "-1");
        assert_eq!(sint_string(&[(0, 0)], 1), "0");
    }
}
