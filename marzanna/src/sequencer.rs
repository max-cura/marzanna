// use chrono::{DateTime, Utc};
// use enum_dispatch::enum_dispatch;
use rand::distr::Distribution;

// use crate::Rendezvous;
// use ortho::Ortho;

pub struct DnsAlphabet;
impl Distribution<u8> for DnsAlphabet {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> u8 {
        // range: a-z0-9\-, since a-z=A-Z since DNS is case-insensitive; size is 36
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        ALPHABET[rng.random_range(0..ALPHABET.len())]
    }
}

// #[enum_dispatch]
// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub enum DynSequencerCore {
//     Ortho,
// }
//
// /// Trait for sequencer implementations. API is unstable. Should only be used by way of a
// /// [`Sequencer`].
// #[enum_dispatch(DynSequencerCore)]
// pub trait SequencerCore: Send {
//     fn materialize_rendezvous_sequence(
//         &mut self,
//         epoch: DateTime<Utc>,
//         count_hint: usize,
//     ) -> Vec<Rendezvous>;
// }

pub mod ortho;

