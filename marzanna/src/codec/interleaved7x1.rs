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
pub struct Interleaved7x1;
impl Interleaved7x1 {
    // Construction: ensure that Hamming(one, zero) is odd for unambiguous decoding
    fn one() -> [bool; 7] {
        [true, false, true, false, true, false, true]
    }
    fn zero() -> [bool; 7] {
        [false, true, false, true, false, true, false]
    }
}
impl Codec for Interleaved7x1 {
    fn required_bits(&self, bytes: usize) -> usize {
        bytes * u8::BITS as usize * 7
    }
    fn encode(&mut self, msg: &Bytes, buf: &mut [bool]) {
        let pre_bits = msg.len() * u8::BITS as usize;
        assert!(buf.len() >= self.required_bits(msg.len()));
        let permute = |i| {
            (i % 7) * pre_bits + (i / 7)
        };
        for (i, b) in msg
            .iter()
            .flat_map(|&b| b.bit_iter_lsb_first())
            .flat_map(|bit| if bit { Self::one() } else { Self::zero() })
            .enumerate()
        {
            buf[permute(i)] = b;
        }
    }

    fn decode(&mut self, msg: &[bool], buf: &mut BytesMut) {
        assert!(
            msg.len().is_multiple_of(7 * 8),
            "msg length must be a multiple of 7x8=40"
        );

        let pre_bits = msg.len() / 7;

        buf.reserve(msg.len() / 7 / 8);
        let mut decoded_bits = vec![];

        // for chunk in msg.chunks_exact(5) {
        //     // PANIC: chunks_exact guarantees length of `chunk` is 3
        //     let chunk_as_array_ref = chunk.as_array().unwrap();
        //     let one_dist = bool_array_hamming(&Self::one(), chunk_as_array_ref);
        //     if one_dist < 3 {
        //         decoded_bits.push(true);
        //     } else {
        //         decoded_bits.push(false);
        //     }
        // }
        for i in 0..pre_bits {
            let chunk = [
                msg[i],
                msg[i + pre_bits],
                msg[i + 2 * pre_bits],
                msg[i + 3 * pre_bits],
                msg[i + 4 * pre_bits],
                msg[i + 5 * pre_bits],
                msg[i + 6 * pre_bits],
            ];
            let one_dist = bool_array_hamming(&Self::one(), &chunk);
            decoded_bits.push(one_dist < 4);
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

// #[test]
// fn s3x1_test_encode() {
//     let msg = Bytes::from_static(&[0b01101001]);
//     let mut v = vec![false; Interleaved5x1.required_bits(msg.len())];
//     Interleaved5x1.encode(&msg, &mut v);
//     assert_eq!(
//         v,
//         [
//             true, false, true,
//             false, true, false,
//             false, true, false,
//             true, false, true,
//             false, true, false,
//             true, false, true,
//             true, false, true,
//             false, true, false,
//         ]
//     );
// }
//
// #[test]
// fn s3x1_test_decode() {
//     let msg = &[
//         true, false, true,
//         false, true, false,
//         false, true, false,
//         true, false, true,
//         false, true, false,
//         true, false, true,
//         true, false, true,
//         false, true, false,
//     ];
//     let mut obuf = BytesMut::new();
//     Interleaved5x1.decode(msg, &mut obuf);
//     assert_eq!(obuf.as_ref(), [0b01101001]);
// }