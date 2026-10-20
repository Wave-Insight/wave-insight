//! Owned signal values.
//!
//! `wellen` hands out borrowed [`wellen::SignalValueRef`]s while it decodes a
//! change. The viewer needs values that outlive the decoder, and it needs to be
//! able to tell a `0`/`1` from an `x`/`z`/`h`/`u`/`w`/`l`/`-` and from a real or
//! string value, so we copy them into [`WaveValue`].
//!
//! Cost: one byte per bit plus one small `Vec` per change. That is *not* the
//! compact form `wellen` keeps internally (2 or 4 bits per bit), but it is only
//! ever created for the signals/queries the caller asked for - see the module
//! documentation of [`crate::waveform`].

use super::hierarchy::SignalKind;
use crate::data_struct::{BitsData, BoolData};

/// A single bit of a two/four/nine state value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitValue {
    /// Strong `0`.
    Zero,
    /// Strong `1`.
    One,
    /// `x` (also `u`, `w` and `-`, which are "unknown" as well).
    X,
    /// `z` (high impedance).
    Z,
    /// `h`, weak high.
    H,
    /// `u`, unresolved.
    U,
    /// `w`, weak unknown.
    W,
    /// `l`, weak low.
    L,
    /// `-`, don't care.
    Dash,
}

impl BitValue {
    /// Parse one VCD/wellen state character (`0 1 x X z Z h H u U w W l L -`).
    pub fn from_ascii(c: char) -> Option<Self> {
        Some(match c {
            '0' => BitValue::Zero,
            '1' => BitValue::One,
            'x' | 'X' => BitValue::X,
            'z' | 'Z' => BitValue::Z,
            'h' | 'H' => BitValue::H,
            'u' | 'U' => BitValue::U,
            'w' | 'W' => BitValue::W,
            'l' | 'L' => BitValue::L,
            '-' => BitValue::Dash,
            _ => return None,
        })
    }

    /// The state character, lower case (`0`, `1`, `x`, `z`, ...).
    pub fn as_char(self) -> char {
        match self {
            BitValue::Zero => '0',
            BitValue::One => '1',
            BitValue::X => 'x',
            BitValue::Z => 'z',
            BitValue::H => 'h',
            BitValue::U => 'u',
            BitValue::W => 'w',
            BitValue::L => 'l',
            BitValue::Dash => '-',
        }
    }

    /// `true` for a strongly driven `0` or `1`.
    pub fn is_known(self) -> bool {
        matches!(self, BitValue::Zero | BitValue::One)
    }

    /// Decode a bit the way the legacy VCD parser did: `h`/`l` count as `1`/`0`
    /// and everything unknown collapses to `x`.
    ///
    /// The old `(u8, u8)` pair is `(xz, num)`; `(0, 0)` = 0, `(0, 1)` = 1,
    /// `(1, 0)` = x, `(1, 1)` = z.
    pub(crate) fn to_legacy_xz(self) -> (u8, u8) {
        match self {
            BitValue::Zero | BitValue::L => (0, 0),
            BitValue::One | BitValue::H => (0, 1),
            BitValue::Z => (1, 1),
            BitValue::X | BitValue::U | BitValue::W | BitValue::Dash => (1, 0),
        }
    }
}

/// An owned bit vector, most significant bit first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bits {
    /// Element 0 is the most significant bit.
    bits: Vec<BitValue>,
}

impl Bits {
    /// Build from bits in most-significant-bit-first order.
    pub fn new_msb_first(bits: Vec<BitValue>) -> Self {
        Self { bits }
    }

    /// A vector of `width` x's (useful for "no value yet" placeholders).
    pub fn all_unknown(width: usize) -> Self {
        Self {
            bits: vec![BitValue::X; width],
        }
    }

    /// Number of bits.
    pub fn width(&self) -> usize {
        self.bits.len()
    }

    /// `true` if the vector has no bits at all.
    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// Bit `index`, where `0` is the least significant bit (like Verilog `value[i]`).
    pub fn get(&self, index: usize) -> Option<BitValue> {
        if index >= self.bits.len() {
            return None;
        }
        Some(self.bits[self.bits.len() - 1 - index])
    }

    /// Iterate from the most significant down to the least significant bit.
    pub fn iter_msb_to_lsb(&self) -> impl Iterator<Item = BitValue> + '_ {
        self.bits.iter().copied()
    }

    /// Iterate from the least significant up to the most significant bit.
    pub fn iter_lsb_to_msb(&self) -> impl Iterator<Item = BitValue> + '_ {
        self.bits.iter().rev().copied()
    }

    /// One character per bit, exactly like a VCD binary literal (`10xz01`).
    pub fn bit_string(&self) -> String {
        let mut out = String::with_capacity(self.bits.len());
        self.write_bit_string(&mut out);
        out
    }

    /// Append the bit string to `out` without allocating a temporary `String`.
    ///
    /// This is what a renderer wants: one reusable buffer for a whole frame
    /// instead of one `String` per drawn value.
    pub fn write_bit_string(&self, out: &mut String) {
        out.reserve(self.bits.len());
        for bit in &self.bits {
            out.push(bit.as_char());
        }
    }

    /// Numeric value if every bit is `0`/`1`, `None` if any bit is unknown or
    /// the vector is wider than 64 bits.
    pub fn as_u64(&self) -> Option<u64> {
        if self.bits.len() > 64 {
            return None;
        }
        let mut out = 0u64;
        for (i, bit) in self.bits.iter().rev().enumerate() {
            match bit {
                BitValue::Zero => {}
                BitValue::One => out |= 1u64 << i,
                _ => return None,
            }
        }
        Some(out)
    }

    /// `true` if at least one bit is not a strong `0`/`1`.
    pub fn has_unknown(&self) -> bool {
        !self.bits.iter().all(|b| b.is_known())
    }

    /// Convert into the old [`BitsData`] so the existing [`ShowType`] renderers
    /// (hex/oct/uint/sint/ascii) can be reused during the UI migration.
    ///
    /// [`ShowType`]: crate::data_struct::ShowType
    pub fn to_bits_data(&self) -> BitsData {
        BitsData::new(self.to_legacy_chunk())
    }

    /// Interpret this as a single bit for the wave view.
    ///
    /// `None` if the vector is not exactly one bit wide. `h`/`l` count as
    /// `1`/`0`, everything else unknown becomes [`BoolData::X`].
    pub fn to_bool_data(&self) -> Option<BoolData> {
        if self.bits.len() != 1 {
            return None;
        }
        Some(match self.bits[0] {
            BitValue::Zero | BitValue::L => BoolData::Zero,
            BitValue::One | BitValue::H => BoolData::One,
            BitValue::Z => BoolData::Z,
            BitValue::X | BitValue::U | BitValue::W | BitValue::Dash => BoolData::X,
        })
    }

    /// Encode into the old `ModuleValue` layout: big endian bytes, one
    /// `(xz, num)` pair per group of eight bits.
    ///
    /// The first `value.len() * 8 - width` bits are padding, which is exactly
    /// what `BitsData::to_string(bitcount, ..)` expects (it drops them).
    pub(crate) fn to_legacy_chunk(&self) -> Vec<(u8, u8)> {
        let width = self.bits.len();
        let n_bytes = width.div_ceil(8);
        let mut out = vec![(0u8, 0u8); n_bytes];
        for (i, bit) in self.bits.iter().rev().enumerate() {
            let byte = n_bytes - 1 - (i / 8);
            let shift = (i % 8) as u8;
            let (xz, num) = bit.to_legacy_xz();
            out[byte].0 |= xz << shift;
            out[byte].1 |= num << shift;
        }
        out
    }
}

/// One value of one signal at one point in time.
///
/// This is the owned, format independent representation; `x`/`z`, real and
/// string values are all preserved.
#[derive(Clone, Debug, PartialEq)]
pub enum WaveValue {
    /// An event happened, there is no payload (VCD `$var event`, FST events).
    Event,
    /// A bit vector, see [`Bits`].
    Bits(Bits),
    /// A floating point value (VCD `$var real`, FST real).
    Real(f64),
    /// A string value (VCD `$var string`, GHW strings, FST variable length).
    String(String),
}

impl WaveValue {
    /// Which kind of payload this value has.
    pub fn kind(&self) -> SignalKind {
        match self {
            WaveValue::Event => SignalKind::Event,
            WaveValue::Bits(_) => SignalKind::BitVector,
            WaveValue::Real(_) => SignalKind::Real,
            WaveValue::String(_) => SignalKind::String,
        }
    }

    /// Bit width for bit vectors and events, `None` for real/string.
    pub fn width(&self) -> Option<usize> {
        match self {
            WaveValue::Bits(bits) => Some(bits.width()),
            WaveValue::Event => Some(0),
            _ => None,
        }
    }

    /// Borrow the bit vector, `None` for real/string/event.
    pub fn as_bits(&self) -> Option<&Bits> {
        match self {
            WaveValue::Bits(bits) => Some(bits),
            _ => None,
        }
    }

    /// `10xz01` for bit vectors, `None` for everything else.
    pub fn to_bit_string(&self) -> Option<String> {
        self.as_bits().map(|bits| bits.bit_string())
    }

    /// Numeric value of a fully defined bit vector.
    pub fn to_u64(&self) -> Option<u64> {
        self.as_bits().and_then(|bits| bits.as_u64())
    }

    /// Convert into the old [`BitsData`] renderer type (for bit vectors).
    pub fn to_bits_data(&self) -> Option<BitsData> {
        self.as_bits().map(Bits::to_bits_data)
    }

    /// Convert into the old [`BoolData`] type (only for one bit values).
    pub fn to_bool_data(&self) -> Option<BoolData> {
        self.as_bits().and_then(Bits::to_bool_data)
    }

    /// `true` if this is an event.
    pub fn is_event(&self) -> bool {
        matches!(self, WaveValue::Event)
    }

    /// Convert into the old `ModuleValue` chunk, if possible.
    ///
    /// Real, string and event values have no representation in the old model
    /// and yield `None`; the legacy adapter skips them.
    pub(crate) fn to_legacy_chunk(&self) -> Option<Vec<(u8, u8)>> {
        match self {
            WaveValue::Bits(bits) => Some(bits.to_legacy_chunk()),
            _ => None,
        }
    }

    /// Copy a value out of `wellen`'s decoder.
    pub(crate) fn from_wellen(value: wellen::SignalValueRef<'_>) -> Self {
        match value {
            wellen::SignalValueRef::Event => WaveValue::Event,
            wellen::SignalValueRef::BitVec(bits) => {
                //`wellen::Bit` is not nameable from outside the crate, but its
                //state characters are part of wellen's public contract
                let bits = bits
                    .iter_msb_to_lsb()
                    .map(|bit| BitValue::from_ascii(bit.as_ascii()).unwrap_or(BitValue::X))
                    .collect();
                WaveValue::Bits(Bits::new_msb_first(bits))
            }
            wellen::SignalValueRef::Real(value) => WaveValue::Real(value),
            wellen::SignalValueRef::String(value) => WaveValue::String(value.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_bit_string_matches_bit_string() {
        let bits = Bits::new_msb_first(vec![
            BitValue::One,
            BitValue::Zero,
            BitValue::X,
            BitValue::Z,
            BitValue::H,
            BitValue::U,
            BitValue::W,
            BitValue::L,
            BitValue::Dash,
        ]);
        assert_eq!(bits.bit_string(), "10xzhuwl-");

        //appending into a reused buffer gives the same text
        let mut buffer = String::from("prefix:");
        bits.write_bit_string(&mut buffer);
        assert_eq!(buffer, format!("prefix:{}", bits.bit_string()));
    }

    #[test]
    fn legacy_chunk_keeps_the_bit_order() {
        //0b1010_1100 as a four state vector, msb first
        let bits =
            Bits::new_msb_first(vec![BitValue::One, BitValue::Zero, BitValue::One, BitValue::Zero]);
        assert_eq!(bits.to_legacy_chunk(), vec![(0, 0b1010)]);
        assert_eq!(bits.to_bits_data().to_string(4, &crate::data_struct::ShowType::Bin), "1010");

        //nine bits: two bytes with the padding in the first one
        let mut msb_first = vec![BitValue::One];
        msb_first.extend(std::iter::repeat(BitValue::Zero).take(8));
        let nine = Bits::new_msb_first(msb_first);
        assert_eq!(nine.to_legacy_chunk(), vec![(0, 0b0000_0001), (0, 0)]);
        assert_eq!(nine.as_u64(), Some(0x100));
        assert_eq!(nine.get(8), Some(BitValue::One));
        assert_eq!(nine.get(0), Some(BitValue::Zero));
    }
}
