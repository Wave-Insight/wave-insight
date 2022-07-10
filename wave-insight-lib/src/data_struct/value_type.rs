
use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ValueType {
    pub value: Vec<u8>,
}

impl ValueType {
    pub fn new(value: Vec<u8>) -> Self {
        Self {
            value
        }
    }

    /// convert u8 to ValueType
    /// 
    /// # Example
    /// 
    /// ```
    /// use wave_insight_lib::data_struct::ValueType;
    /// let value = 4u8;
    /// let convert = ValueType::from_u8(value);
    /// assert!(convert == ValueType{value:vec![4]});
    /// ```
    pub fn from_u8(value: u8) -> Self {
        Self {
            value: vec![value]
        }
    }

    /// convert binary string to ValueType
    /// 
    /// # Example
    /// 
    /// ```
    /// use wave_insight_lib::data_struct::ValueType;
    /// let input = "01011101101";
    /// let convert = ValueType::parse_bin_string(input);
    /// assert!(convert == ValueType{value:vec![2, 237]})
    /// ```
    pub fn parse_bin_string(input: &str) -> Self {//TODO:perf is bad
        let length = input.len();
        let input_head = if length%8 == 0 {""}
            else if length%8 == 1 {"0000000"}
            else if length%8 == 2 {"000000"}
            else if length%8 == 3 {"00000"}
            else if length%8 == 4 {"0000"}
            else if length%8 == 5 {"000"}
            else if length%8 == 6 {"00"}
            else {"0"};
        let input_convert = input_head.to_string() + &input.to_string();
        let ret = input_convert.as_bytes()
            .chunks(8)
            .map(|x| x.iter().fold(0, |a,&b| 2*a+(b-48)));
        let drop_prefix_zero: Vec<u8> = ret.skip_while(|&x| x == 0).collect();
        Self { value: drop_prefix_zero }
    }

    /// convert ValueType to hex string
    /// 
    /// # Example
    /// 
    /// ```
    /// use wave_insight_lib::data_struct::ValueType;
    /// let input = "01011101101";
    /// let convert = ValueType::parse_bin_string(input);
    /// let hex3 = convert.to_hex_string(3);
    /// assert!(hex3 == "2ED".to_string());
    /// let hex4 = convert.to_hex_string(4);
    /// assert!(hex4 == "02ED".to_string());
    /// let hex5 = convert.to_hex_string(5);
    /// assert!(hex5 == "002ED".to_string());
    /// ```
    pub fn to_hex_string(&self, width: usize) -> String {//TODO:maybe the first char should be drop
        let origin_string = (&self.value).iter()
            .map(|&x| {
                let high = x/16;
                let low = x%16;
                let to_str = |y: u8| if y >= 10 {y - 10 + b'A'} else {y + b'0'};
                unsafe{ String::from_utf8_unchecked(vec![to_str(high),to_str(low)]) }
            }).reduce(|a,b| a+&b).unwrap_or_else(|| "".to_string());//hex::encode_upper(&self.value);
        match (self.value.len()*2).cmp(&width) {
            Ordering::Less => {
                (0..(width-self.value.len()*2)).map(|_x| "0".to_string())
                .reduce(|a,b| a+&b)
                .unwrap() + &origin_string
            },
            Ordering::Equal => origin_string,
            Ordering::Greater => origin_string.split_at(1).1.to_string()
        }
    }

    pub fn to_oct_string(&self) -> String {
        hex::encode_upper(&self.value)//TODO:oct instead of hex
    }

    pub fn to_bin_string(&self) -> String {
        hex::encode_upper(&self.value)//TODO:bin instead of hex
    }

    pub fn to_uint_string(&self) -> String {
        hex::encode_upper(&self.value)//TODO:uint instead of hex
    }

    pub fn to_sint_string(&self, width: usize) -> String {
        hex::encode_upper(&self.value)//TODO:sint instead of hex
    }

    pub fn to_ascii_string(&self) -> String {
        let s = match std::str::from_utf8(&self.value){
            Ok(v) => v,
            Err(_e) => "invalid"
        };
        s.to_string()
        //unsafe{ String::from_utf8_unchecked(self.value.clone()) }
    }

}

/*use num::{ BigUint, BigInt, bigint::{ToBigInt, Sign} };

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ValueType {
    pub value: BigUint,
}

impl ValueType {
    pub fn new(value: Vec<u8>) -> Self {
        Self {
            value: BigUint::from_bytes_be(&value)
        }
    }
    pub fn from_u8(value: u8) -> Self {
        Self::new(vec![value])
    }
    pub fn parse_bin_string(input: &str) -> Self {//TODO:perf is bad
        Self { value: BigUint::parse_bytes(input.as_bytes(), 2).unwrap() }
    }
    #[inline]
    pub fn to_hex_string(&self, width: usize) -> String {//TODO:maybe the first char should be drop
        let origin_string = self.value.to_str_radix(16);
        match (origin_string.len()).cmp(&width) {
            Ordering::Less => {
                (0..(width-origin_string.len())).map(|_x| "0".to_string())
                .reduce(|a,b| a+&b)
                .unwrap() + &origin_string
            },
            Ordering::Equal => origin_string,
            Ordering::Greater => origin_string.split_at(origin_string.len()-width).1.to_string()//TODO:no possible
        }
    }
    #[inline]
    pub fn to_oct_string(&self) -> String {
        self.value.to_str_radix(8)
    }
    #[inline]
    pub fn to_bin_string(&self) -> String {
        self.value.to_str_radix(2)
    }
    #[inline]
    pub fn to_uint_string(&self) -> String {
        format!("{}",self.value)
    }
    #[inline]
    pub fn to_sint_string(&self, width: u32) -> String {
        let bound = BigUint::new(vec![2]).pow(width-1);
        let value_to_sint = if self.value >= bound {
            self.value.to_bigint().unwrap() - BigInt::new(Sign::Plus,vec![2]).pow(width)
        }else {
            self.value.to_bigint().unwrap()
        };
        format!("{}",value_to_sint)
    }
    #[inline]
    pub fn to_ascii_string(&self) -> String {
        let value_to_bytes = self.value.to_bytes_be();
        let s = match std::str::from_utf8(&value_to_bytes) {
            Ok(v) => v,
            Err(_e) => "invalid",
        };
        s.to_string()
    }
}*/
