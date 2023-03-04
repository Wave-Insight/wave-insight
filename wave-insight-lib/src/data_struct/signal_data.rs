use num::{BigUint, BigInt, bigint::{ToBigInt, Sign}};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub enum BoolData {
    Zero,
    One,
    X,
    Z,
}

impl BoolData {
    pub fn new(value: (u8, u8)) -> Self {
        match value {
            (0, 0) => BoolData::Zero,
            (0, 1) => BoolData::One,
            (1, 0) => BoolData::X,
            _ => BoolData::Z,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct BitsData {
                 // xz,number
    value: Vec<(u8,u8)>,//when xz == 1, num == 0 -> x and num == 1 -> z
}

impl BitsData {
    pub fn new(value: Vec<(u8,u8)>) -> Self {
        Self { value }
    }
    pub fn to_string(&self, bitcount: usize, showtype: &ShowType) -> String {
        match showtype {
            ShowType::Hex => self.hex_string(bitcount),
            ShowType::UInt => self.uint_string(),
            ShowType::SInt => self.sint_string(bitcount),
            ShowType::Oct => self.oct_string(bitcount),
            ShowType::Bin => self.bin_string(bitcount),
            ShowType::Ascii => self.asc_string(),
        }
    }
    fn bin_string(&self, bitcount: usize) -> String {
        let drop_head = self.value.len() * 8 - bitcount;
        let ret: Vec<u8> = self.value.iter().flat_map(|(xz, num)| {
            (0..8).rev().map(move |idx| match (xz & (1 << idx), num & (1 << idx)) {
                (0, 0) => b'0',
                (0, _) => b'1',
                (_, 0) => b'x',
                (_, _) => b'z',
            })
        }).skip(drop_head).collect();
        String::from_utf8(ret).unwrap_or("error".to_owned())//TODO:use from_utf8_unchecked?
    }
    fn oct_string(&self, bitcount: usize) -> String {
        //TODO: for something like 0zx, gtkwave will show z only in oct, while this will show x both in hex and oct
        let drop_head = ((self.value.len()*8+2)/3) - ((bitcount+2)/3);
        let mut state = (self.value.len() % 3) as u8;
        let mut pad_head = (0u8, 0u8);
        let ret: Vec<u8> = self.value.iter()
            .flat_map(|(xz, num)| {
                if state == 0 {
                    state = 2;
                    let ret = vec![
                        (xz >> 5, num >> 5),
                        ((xz>>2) & 0b00000111, (num>>2) & 0b00000111)
                    ];
                    pad_head = (xz & 0b00000011, num & 0b00000011);
                    ret
                }else if state == 1 {
                    state = 0;
                    let ret = vec![
                        ((pad_head.0 << 2) + (xz >> 6), (pad_head.1 << 2) + (num >> 6)),
                        ((xz>>3) & 0b00000111, (num>>3) & 0b00000111),
                        (xz & 0b00000111, num & 0b00000111)
                    ];
                    pad_head = (0, 0);
                    ret
                }else {
                    state = 1;
                    let ret = vec![
                        ((pad_head.0 << 1) + (xz >> 7), (pad_head.1 << 1) + (num >> 7)),
                        ((xz>>4) & 0b00000111, (num>>4) & 0b00000111),
                        ((xz>>1) & 0b00000111, (num>>1) & 0b00000111)
                    ];
                    pad_head = (xz & 0b00000001, num & 0b00000001);
                    ret
                }
            }).map(|(xz, num)| match (xz, num) {
                (0, _) => b'0' + num,
                _ => if xz & (!num) == 0 {b'z'} else {b'x'}
            }).skip(drop_head).collect();
        String::from_utf8(ret).unwrap_or("error".to_owned())//TODO:use from_utf8_unchecked?
    }
    fn hex_string(&self, bitcount: usize) -> String {
        let drop_head = if (self.value.len() * 8 - bitcount) >= 4 {1} else {0};
        let convert = |xz: u8, num: u8| match (xz, num) {
                (0, _) => if num < 10 {b'0' + num} else {b'a' - 10 + num}
                (_, _) => if xz & (!num) == 0 {b'z'} else {b'x'}
            };
        let ret: Vec<u8> = self.value.iter()
            .flat_map(|(xz, num)| [(xz >> 4, num >> 4), (xz & 0b00001111, num & 0b00001111)])
            .map(|(xz, num)| convert(xz, num))
            .skip(drop_head).collect();
        String::from_utf8(ret).unwrap_or("error".to_owned())//TODO:use from_utf8_unchecked?
    }
    fn uint_string(&self) -> String {
        let has_xz = self.value.iter()
            .map(|(xz, _)| xz)
            .any(|&xz| xz != 0);
        if has_xz {
            "X".to_owned()//TODO:more 'X' base on size?
        }else {
            let num: Vec<u8> = self.value.iter()
                .map(|(_, num)| *num)
                .collect();
            let value = BigUint::from_bytes_be(&num);
            format!("{value}")
        }
    }
    fn sint_string(&self, bitcount: usize) -> String {
        let has_xz = self.value.iter()
            .map(|(xz, _)| xz)
            .any(|&xz| xz != 0);
        if has_xz {
            "X".to_owned()//TODO:more 'X' base on size?
        }else {
            let num: Vec<u8> = self.value.iter()
                .map(|(_, num)| *num)
                .collect();
            let value = BigUint::from_bytes_be(&num);
            let bound = BigUint::new(vec![2]).pow((bitcount as u32)-1);
            let value_to_sint = if value >= bound {
                value.to_bigint().unwrap() - BigInt::new(Sign::Plus,vec![2]).pow(bitcount as u32)
            }else {
                value.to_bigint().unwrap()
            };
            format!("{value_to_sint}")
        }
    }
    fn asc_string(&self) -> String {
        //TODO:when something like 0u8, now thowed, need something to show there is something
        let vec = self.value.iter()
            .map(|(xz, num)| if *xz == 0 {*num} else {0})
            .collect();
        String::from_utf8(vec).unwrap_or("invalid".to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ShowType {
    Hex,
    UInt,
    SInt,
    Oct,
    Bin,
    Ascii,
}

#[test]
fn test() {
    assert_eq!(BitsData::new(vec![(0,1),(0,23)]).bin_string(10), "0100010111".to_owned());
    assert_eq!(BitsData::new(vec![(0,1),(0,123)]).bin_string(10), "0101111011".to_owned());
    assert_eq!(BitsData::new(vec![(1,1),(0,123)]).bin_string(10), "0z01111011".to_owned());
    assert_eq!(BitsData::new(vec![(1,32),(0,123)]).bin_string(14), "10000x01111011".to_owned());
    assert_eq!(BitsData::new(vec![(1,2),(6,123)]).bin_string(15), "000001x01111xz1".to_owned());

    assert_eq!(BitsData::new(vec![(0,1),(0,23)]).oct_string(10), "0427".to_owned());
    assert_eq!(BitsData::new(vec![(0,1),(0,123)]).oct_string(10), "0573".to_owned());
    assert_eq!(BitsData::new(vec![(1,1),(0,123)]).oct_string(10), "0z73".to_owned());
    assert_eq!(BitsData::new(vec![(1,32),(0,123)]).oct_string(14), "20x73".to_owned());
    assert_eq!(BitsData::new(vec![(1,2),(6,123)]).oct_string(15), "01x7x".to_owned());

    assert_eq!(BitsData::new(vec![(0,1),(0,23)]).hex_string(10), "117".to_owned());
    assert_eq!(BitsData::new(vec![(0,1),(0,123)]).hex_string(10), "17b".to_owned());
    assert_eq!(BitsData::new(vec![(1,1),(0,123)]).hex_string(10), "z7b".to_owned());
    assert_eq!(BitsData::new(vec![(1,32),(0,123)]).hex_string(14), "2x7b".to_owned());
    assert_eq!(BitsData::new(vec![(1,2),(6,123)]).hex_string(15), "0x7x".to_owned());

    assert_eq!(BitsData::new(vec![(0,1),(0,23)]).uint_string(), "279".to_owned());
    assert_eq!(BitsData::new(vec![(1,1),(0,23)]).uint_string(), "X".to_owned());

    assert_eq!(BitsData::new(vec![(0,1),(0,23)]).sint_string(10), "279".to_owned());
    assert_eq!(BitsData::new(vec![(0,2),(0,23)]).sint_string(10), "-489".to_owned());
    assert_eq!(BitsData::new(vec![(1,1),(0,23)]).sint_string(10), "X".to_owned());
}
