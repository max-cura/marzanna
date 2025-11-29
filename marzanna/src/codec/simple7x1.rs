use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use crate::{bits::IntoBitIter as _, codec::Codec};

fn bool_array_hamming<const N: usize>(lhs: &[bool; N], rhs: &[bool; N]) -> usize {
    lhs.iter()
        .zip(rhs.iter())
        .map(|(l, r)| l ^ r)
        .filter(|x| *x)
        .count()
}

/// Very simple FEC; take two bitstrings with high and odd Hamming distance, assign one as 1 and
/// the other as 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Simple7x1;
impl Simple7x1 {
    // Construction: ensure that Hamming(one, zero) is odd for unambiguous decoding
    fn one() -> [bool; 7] {
        [true, false, true, false, true, false, true]
    }
    fn zero() -> [bool; 7] {
        [false, true, false, true, false, true, false]
    }
}
impl Codec for Simple7x1 {
    fn required_bits(&self, msg: &Bytes) -> usize {
        msg.len() * u8::BITS as usize * 7
    }
    fn encode(&mut self, msg: &Bytes, buf: &mut [bool]) {
        assert!(buf.len() >= self.required_bits(&msg));
        for (i, b) in msg
            .iter()
            .flat_map(|&b| b.bit_iter_lsb_first())
            .flat_map(|bit| if bit { Self::one() } else { Self::zero() })
            .enumerate()
        {
            buf[i] = b;
        }
    }

    fn decode(&mut self, msg: &[bool], buf: &mut BytesMut) {
        assert!(
            msg.len().is_multiple_of(7 * 8),
            "msg length must be a multiple of 7x8=56"
        );

        buf.reserve(msg.len() / 56);
        let mut decoded_bits = vec![false; msg.len() / 7];

        for chunk in msg.chunks_exact(7) {
            // PANIC: chunks_exact guarantees length of `chunk` is 7
            let chunk_as_array_ref = chunk.as_array().unwrap();
            let one_dist = bool_array_hamming(&Self::one(), chunk_as_array_ref);
            // one_dist is [0, 7]
            //            one    zero
            // 0101010  - dist=7 dist=0
            // 010101 1 - dist=6 dist=1
            // 01010 01 - dist=5 dist=2
            // 0101 101 - dist=4 dist=3
            // 010 0101 - dist=3 dist=4
            // 01 10101 - dist=2 dist=5
            // 0 010101 - dist=1 dist=6
            //  1010101 - dist=0 dist=7
            if one_dist < 4 {
                decoded_bits.push(true);
            } else {
                decoded_bits.push(false);
            }
        }

        for chunk in decoded_bits.chunks_exact(8) {
            // PANIC: chunks_exact guarantess length of `chunk` is 8
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

#[test]
fn s7x1_test_encode() {
    let msg = Bytes::from_static(&[0b01101001]);
    let mut v = vec![false; Simple7x1.required_bits(&msg)];
    Simple7x1.encode(&msg, &mut v);
    assert_eq!(
        v,
        [
            true, false, true, false, true, false, true, // 1
            false, true, false, true, false, true, false, // 0
            false, true, false, true, false, true, false, // 0
            true, false, true, false, true, false, true, // 1
            false, true, false, true, false, true, false, // 0
            true, false, true, false, true, false, true, // 1
            true, false, true, false, true, false, true, // 1
            false, true, false, true, false, true, false, // 0
        ]
    );
}
