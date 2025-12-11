//! `libmarzanna` provides the functionality of marzanna as a library.
//!
//! This crate doesn't handle any of the actual networking that happens, only the protocol level
//! state and mechanisms.
#![feature(slice_as_array)]
#![feature(btree_cursors)]

use chrono::{DateTime, Utc};
use hostaddr::{Buffer, Domain};
use serde::{Deserialize, Serialize};

use crate::sequencer::Timepoint;

pub mod bits;
pub mod codec;
pub mod protocol;
pub mod sequencer;
mod serde_chacha;
pub mod session;

#[derive(Clone, Serialize, Deserialize)]
pub struct Rendezvous {
    idx: usize,
    host: Domain<Buffer>,
    timepoint: Timepoint,
}
impl std::fmt::Debug for Rendezvous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rendezvous")
            .field("idx", &self.idx)
            .field("host", &format!("{}", self.host))
            .field(
                "write",
                &format!(
                    "{}-{}",
                    self.timepoint.can_write_at, self.timepoint.write_by
                ),
            )
            .field(
                "read",
                &format!("{}-{}", self.timepoint.can_read_at, self.timepoint.read_by),
            )
            .finish()
    }
}
impl Rendezvous {
    pub fn idx(&self) -> usize {
        self.idx
    }
    pub fn domain(&self) -> &Domain<Buffer> {
        &self.host
    }
    pub fn can_write_at(&self) -> DateTime<Utc> {
        self.timepoint.can_write_at
    }
    pub fn write_by(&self) -> DateTime<Utc> {
        self.timepoint.write_by
    }
    pub fn can_read_at(&self) -> DateTime<Utc> {
        self.timepoint.can_read_at
    }
    pub fn read_by(&self) -> DateTime<Utc> {
        self.timepoint.read_by
    }
}

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct Sequencer {
//     time: DynTimeSequencer,
//     domain: DynDomainSequencer,
//     idx: usize,
//     materialized_time_cache: VecDeque<[DateTime<Utc>; 4]>,
//     materialized_domain_cache: VecDeque<(Domain<Buffer>, u64)>,
// }
// impl Sequencer {
//     pub fn new(time: DynTimeSequencer, domain: DynDomainSequencer) -> Self {
//         Self {
//             time,
//             domain,
//             idx: 0,
//             materialized_time_cache: Default::default(),
//             materialized_domain_cache: Default::default(),
//         }
//     }
//     pub fn idx(&mut self) -> usize {
//         self.idx
//     }
//     pub fn next_idx(&mut self) {
//         self.idx += 1;
//     }
//     pub fn peek_time(&mut self, epoch: DateTime<Utc>) -> [DateTime<Utc>; 4] {
//         if let Some(r) = self.materialized_time_cache.front() {
//             r.clone()
//         } else {
//             self.materialize_time(epoch);
//             self.materialized_time_cache.front().unwrap().clone()
//         }
//     }
//     pub fn peek_domain(&mut self) -> (Domain<Buffer>, u64) {
//         if let Some(r) = self.materialized_domain_cache.front() {
//             r.clone()
//         } else {
//             self.materialize_domain();
//             self.materialized_domain_cache.front().unwrap().clone()
//         }
//     }
//     pub fn pop_time(&mut self) {
//         self.materialized_time_cache.pop_front().unwrap();
//     }
//     pub fn pop_domain(&mut self) -> (Domain<Buffer>, u64) {
//         self.materialized_domain_cache.pop_front().unwrap()
//     }
//     fn materialize_time(&mut self, epoch: DateTime<Utc>) {
//         self.materialized_time_cache
//             .push_back(self.time.next_time(epoch));
//     }
//     fn materialize_domain(&mut self) {
//         self.materialized_domain_cache
//             .push_back(self.domain.next_domain());
//     }
// }
