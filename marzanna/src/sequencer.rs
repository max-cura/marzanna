use rand::distr::Distribution;

use chrono::{DateTime, Utc};
use enum_dispatch::enum_dispatch;
use hostaddr::{Buffer, Domain};
use serde::{Deserialize, Serialize};

pub mod domain;
pub mod time;

use domain::UniformNSL;
use domain::WeightedList;
use time::UniformOffset;

pub struct DnsAlphabet;
impl Distribution<u8> for DnsAlphabet {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> u8 {
        // range: a-z0-9\-, since a-z=A-Z since DNS is case-insensitive; size is 36
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        ALPHABET[rng.random_range(0..ALPHABET.len())]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timepoint {
    pub can_write_at: DateTime<Utc>,
    pub write_by: DateTime<Utc>,
    pub can_read_at: DateTime<Utc>,
    pub read_by: DateTime<Utc>,
}

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name")]
pub enum DynTimeSequencer {
    UniformOffset,
}
#[enum_dispatch(DynTimeSequencer)]
pub trait TimeSequencer: Send {
    fn next_time(&mut self, epoch: DateTime<Utc>) -> Timepoint;
}

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name")]
pub enum DynDomainSequencer {
    UniformNSL,
    WeightedList,
}
#[enum_dispatch(DynDomainSequencer)]
pub trait DomainSequencer: Send {
    fn next_domain(&mut self) -> (Domain<Buffer>, u64);
}
