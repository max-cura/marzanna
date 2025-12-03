use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use crate::bits::IntoBitIter;
use crate::codec::Codec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Simple1x1;
impl Codec for Simple1x1 {
    fn required_bits(&self, bytes: usize) -> usize {
        bytes * u8::BITS as usize
    }

    fn encode(&mut self, msg: &Bytes, buf: &mut [bool]) {
        assert!(buf.len() >= self.required_bits(msg.len()));
        for (i, b) in msg.iter().flat_map(|&b| b.bit_iter_lsb_first())
            .enumerate()
        {
            buf[i] = b;
        }
    }

    fn decode(&mut self, msg: &[bool], buf: &mut BytesMut) {
        assert!(
            msg.len().is_multiple_of(8),
            "msg length must be a multiple of 8"
        );
        buf.reserve(msg.len() / 8);
        for chunk in msg.chunks_exact(8) {
            let chunk_as_array_ref: &[bool; 8] = chunk.as_array().unwrap();
            let mut byte: u8 = 0;
            // LSB-first
            for bit in chunk_as_array_ref {
                byte >>= 1;
                byte |= if *bit { 0x80 } else { 0 };
            }
            buf.put_u8(byte);
        }
    }
}