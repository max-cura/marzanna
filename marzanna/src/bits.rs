pub struct BitIterU8(u16);

impl Iterator for BitIterU8 {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        let [low, high] = self.0.to_le_bytes();
        self.0 = (self.0 >> 1) & 0xff7f;
        if high == 0 {
            None
        } else {
            Some((low & 1) != 0)
        }
    }
}

pub trait IntoBitIter {
    type Iter: Iterator<Item = bool>;

    fn bit_iter_lsb_first(self) -> Self::Iter;
    #[allow(unused)]
    fn bit_iter_msb_first(self) -> Self::Iter;
}

impl IntoBitIter for u8 {
    type Iter = BitIterU8;

    fn bit_iter_lsb_first(self) -> Self::Iter {
        BitIterU8((self as u16) | 0xff00)
    }
    fn bit_iter_msb_first(self) -> Self::Iter {
        BitIterU8((self.reverse_bits() as u16) | 0xff00)
    }
}

#[test]
fn test_bit_iter_lsb_first() {
    let v: Vec<bool> = 0b1010_0011.bit_iter_lsb_first().collect();
    assert_eq!(v, &[true, true, false, false, false, true, false, true]);
}
#[test]
fn test_bit_iter_msb_first() {
    let v: Vec<bool> = 0b1010_0011.bit_iter_msb_first().collect();
    assert_eq!(v, &[true, false, true, false, false, false, true, true]);
}
