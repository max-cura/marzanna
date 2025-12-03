use bytes::{Bytes, BytesMut};
use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};

mod simple7x1;
pub use simple7x1::Simple7x1;
mod simple3x1;
pub use simple3x1::Simple3x1;
mod simple1x1;
pub use simple1x1::Simple1x1;
mod simple5x1;
pub use simple5x1::Simple5x1;
mod interleaved5x1;
pub use interleaved5x1::Interleaved5x1;
mod interleaved7x1;
pub use interleaved7x1::Interleaved7x1;
mod interleaved3x1;
pub use interleaved3x1::Interleaved3x1;

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name")]
pub enum DynCodec {
    Simple7x1,
    Simple5x1,
    Simple3x1,
    Simple1x1,
    Interleaved3x1,
    Interleaved5x1,
    Interleaved7x1,
}

#[enum_dispatch(DynCodec)]
pub trait Codec {
    fn required_bits(&self, bytes: usize) -> usize;
    fn encode(&mut self, msg: &Bytes, buf: &mut [bool]);
    fn decode(&mut self, msg: &[bool], buf: &mut BytesMut);
}
