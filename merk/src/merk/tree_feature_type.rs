use std::io::{Read, Write};
use byteorder::{BigEndian, ReadBytesExt};
use ed::{Encode, Terminated, Result, Decode};
use ed::Error::UnexpectedByte;
use crate::merk::tree_feature_type::TreeFeatureType::{BasicMerk, SummedMerk};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TreeFeatureType {
    BasicMerk,
    SummedMerk(u64)
}

impl Terminated for TreeFeatureType {

}

impl Encode for TreeFeatureType {
    #[inline]
    fn encode_into<W: Write>(&self, out: &mut W) -> Result<()> {
        match self {
            BasicMerk => {
                out.write_all(&[0 as u8])?;
            }
            SummedMerk(sum) => {
                out.write_all(&[1 as u8])?;
                out.write_all(sum.to_be_bytes().as_slice())?;
            }
        }
        Ok(())
    }

    #[inline]
    fn encoding_length(&self) -> Result<usize> {
        Ok(match self {
            BasicMerk => { 1}
            SummedMerk(_) => { 9}
        })
    }
}

impl Decode for TreeFeatureType {
    #[inline]
    fn decode<R: Read>(mut input: R) -> Result<Self> {
        let feature_type = input.read_u8()?;

        match feature_type {
            0 => { Ok(BasicMerk) },
            1 => {
                let sum = input.read_u64::<BigEndian>()?;
                Ok(SummedMerk(sum))
            }
            _ => Err(UnexpectedByte(feature_type))
        }
    }
}