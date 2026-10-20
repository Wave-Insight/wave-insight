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

    /// The single bit of a one bit vector, `None` for wider vectors.
    ///
    /// This is what the wave view draws a digital step from.
    pub fn single_bit(&self) -> Option<BitValue> {
        if self.bits.len() == 1 {
            Some(self.bits[0])
        } else {
            None
        }
    }

    /// Render with one of the viewer's [`ShowType`]s (hex, oct, decimal, ...).
    ///
    /// `bitcount` is the number of bits the *display* has room for; it is
    /// clamped to this vector's width, so a dump that is narrower than the
    /// design cannot underflow the formatter (it used to panic with "attempt to
    /// subtract with overflow") and a wider display does not show padding zeros.
    ///
    /// [`ShowType`]: crate::data_struct::ShowType
    pub fn render(&self, bitcount: usize, show: &crate::data_struct::ShowType) -> String {
        let bitcount = bitcount.min(self.bits.len());
        super::render::render_chunk(&self.to_chunk(), bitcount, show)
    }

    /// Encode into the `(xz, num)` byte layout the formatter works on: big
    /// endian bytes, one pair per group of eight bits.
    ///
    /// The leading `chunk.len() * 8 - width` bits are padding, which is exactly
    /// what the formatter drops when `bitcount` is smaller.
    pub(crate) fn to_chunk(&self) -> Vec<(u8, u8)> {
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

    /// The inverse of [`Bits::to_chunk`], for tests: the low `width` bits of a
    /// packed byte layout.
    #[cfg(test)]
    pub(crate) fn from_chunk(chunk: &[(u8, u8)], width: usize) -> Self {
        let mut bits = Vec::with_capacity(chunk.len() * 8);
        for (xz, num) in chunk {
            for idx in (0..8).rev() {
                let x = xz & (1 << idx) != 0;
                let n = num & (1 << idx) != 0;
                bits.push(match (x, n) {
                    (false, false) => BitValue::Zero,
                    (false, true) => BitValue::One,
                    (true, false) => BitValue::X,
                    (true, true) => BitValue::Z,
                });
            }
        }
        let width = width.min(bits.len());
        Bits::new_msb_first(bits[bits.len() - width..].to_vec())
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

    /// Render this value with one of the viewer's [`ShowType`]s.
    ///
    /// `None` for real, string and event values: they are not bit vectors.
    /// `bitcount` is clamped to the value's width (see [`Bits::render`]).
    ///
    /// [`ShowType`]: crate::data_struct::ShowType
    pub fn render(&self, bitcount: usize, show: &crate::data_struct::ShowType) -> Option<String> {
        self.as_bits().map(|bits| bits.render(bitcount, show))
    }

    /// `true` if this is an event.
    pub fn is_event(&self) -> bool {
        matches!(self, WaveValue::Event)
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
    fn chunk_keeps_the_bit_order() {
        //0b1010 as a four state vector, msb first
        let bits =
            Bits::new_msb_first(vec![BitValue::One, BitValue::Zero, BitValue::One, BitValue::Zero]);
        assert_eq!(bits.to_chunk(), vec![(0, 0b1010)]);
        assert_eq!(bits.render(4, &crate::data_struct::ShowType::Bin), "1010");

        //nine bits: two bytes with the padding in the first one
        let mut msb_first = vec![BitValue::One];
        msb_first.extend(std::iter::repeat(BitValue::Zero).take(8));
        let nine = Bits::new_msb_first(msb_first);
        assert_eq!(nine.to_chunk(), vec![(0, 0b0000_0001), (0, 0)]);
        assert_eq!(nine.as_u64(), Some(0x100));
        assert_eq!(nine.get(8), Some(BitValue::One));
        assert_eq!(nine.get(0), Some(BitValue::Zero));
    }

    /// The packed layout and the bit vector must round trip, because the
    /// formatter works on the packed layout. (The packing is four state, so this
    /// only holds for 0/1/x/z values; `h`/`l`/`u`/`w`/`-` are lossy by design.)
    #[test]
    fn from_chunk_is_the_inverse_of_to_chunk() {
        for width in 1..=20usize {
            let bits: Vec<BitValue> = (0..width)
                .map(|index| match index % 4 {
                    0 => BitValue::Zero,
                    1 => BitValue::One,
                    2 => BitValue::X,
                    _ => BitValue::Z,
                })
                .collect();
            let bits = Bits::new_msb_first(bits);
            let round = Bits::from_chunk(&bits.to_chunk(), width);
            assert_eq!(round.bit_string(), bits.bit_string(), "width {width}");
        }
    }

    /// `Bits::render` is the `ShowType` formatter; it must agree with the old
    /// `BitsData::to_string(bitcount, showtype)` for every value the viewer
    /// draws. These are the old expectations, expressed as bit vectors.
    #[test]
    fn render_matches_the_old_formatter() {
        use crate::data_struct::ShowType;
        let two = Bits::from_chunk(&[(0, 1), (0, 23)], 10);
        assert_eq!(two.render(10, &ShowType::Bin), "0100010111");
        assert_eq!(two.render(10, &ShowType::Oct), "0427");
        assert_eq!(two.render(10, &ShowType::Hex), "117");
        assert_eq!(two.render(10, &ShowType::UInt), "279");
        assert_eq!(two.render(10, &ShowType::SInt), "279");

        let z = Bits::from_chunk(&[(1, 1), (0, 123)], 10);
        assert_eq!(z.render(10, &ShowType::Bin), "0z01111011");
        assert_eq!(z.render(10, &ShowType::Hex), "z7b");
        assert_eq!(z.render(10, &ShowType::UInt), "X");

        //a value narrower than the display width is clamped, not a panic
        let narrow = Bits::from_chunk(&[(0, 0x18)], 5);
        assert_eq!(narrow.render(32, &ShowType::Hex), "18");
        assert_eq!(narrow.render(32, &ShowType::Bin), "11000");
    }
}
