//! `libmarzanna` provides the functionality of marzanna as a library.
//!
//! This crate doesn't handle any of the actual networking that happens, only the protocol level
//! state and mechanisms.
#![feature(slice_as_array)]
#![feature(btree_cursors)]

use crate::sequencer::ortho::{DomainSequencer, TimeSequencer};
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use hostaddr::{Buffer, Domain};
use serde::{Deserialize, Serialize};

pub mod bits;
pub mod codec;
pub mod protocol;
pub mod sequencer;
mod serde_chacha;
pub mod session;

use crate::sequencer::ortho::{DynDomainSequencer, DynTimeSequencer};

#[derive(Clone, Serialize, Deserialize)]
pub struct Rendezvous {
    idx: usize,
    host: Domain<Buffer>,
    can_write_at: DateTime<Utc>,
    write_by: DateTime<Utc>,
    can_read_at: DateTime<Utc>,
    read_by: DateTime<Utc>,
    cache_ttl: u64,
}
impl std::fmt::Debug for Rendezvous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rendezvous")
            .field("idx", &self.idx)
            .field("host", &format!("{}", self.host))
            .field("write", &format!("{}-{}", self.can_write_at, self.write_by))
            .field("read", &format!("{}-{}", self.can_read_at, self.read_by))
            .field("cache_ttl", &self.cache_ttl)
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
        self.can_write_at
    }
    pub fn write_by(&self) -> DateTime<Utc> {
        self.write_by
    }
    pub fn can_read_at(&self) -> DateTime<Utc> {
        self.can_read_at
    }
    pub fn read_by(&self) -> DateTime<Utc> {
        self.read_by
    }
    pub fn cache_ttl(&self) -> u64 {
        self.cache_ttl
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequencer {
    time: DynTimeSequencer,
    domain: DynDomainSequencer,
    idx: usize,
    materialized_time_cache: VecDeque<[DateTime<Utc>; 4]>,
    materialized_domain_cache: VecDeque<(Domain<Buffer>, u64)>,
}
impl Sequencer {
    pub fn new(time: DynTimeSequencer, domain: DynDomainSequencer) -> Self {
        Self {
            time,
            domain,
            idx: 0,
            materialized_time_cache: Default::default(),
            materialized_domain_cache: Default::default(),
        }
    }
    pub fn idx(&mut self) -> usize {
        self.idx
    }
    pub fn next_idx(&mut self) {
        self.idx += 1;
    }
    pub fn peek_time(&mut self, epoch: DateTime<Utc>) -> [DateTime<Utc>; 4] {
        if let Some(r) = self.materialized_time_cache.front() {
            r.clone()
        } else {
            self.materialize_time(epoch);
            self.materialized_time_cache.front().unwrap().clone()
        }
    }
    pub fn peek_domain(&mut self) -> (Domain<Buffer>, u64) {
        if let Some(r) = self.materialized_domain_cache.front() {
            r.clone()
        } else {
            self.materialize_domain();
            self.materialized_domain_cache.front().unwrap().clone()
        }
    }
    pub fn pop_time(&mut self) {
        self.materialized_time_cache.pop_front().unwrap();
    }
    pub fn pop_domain(&mut self) -> (Domain<Buffer>, u64) {
        self.materialized_domain_cache.pop_front().unwrap()
    }
    fn materialize_time(&mut self, epoch: DateTime<Utc>) {
        self.materialized_time_cache
            .push_back(self.time.next_time(epoch));
    }
    fn materialize_domain(&mut self) {
        self.materialized_domain_cache
            .push_back(self.domain.next_domain());
    }
}

// #[derive(Debug, Clone)]
// pub struct Sequencer {
//     inner: DynSequencerCore,
//     materialized_cache: VecDeque<Rendezvous>,
// }
// impl Sequencer {
//     pub(crate) fn new(inner: DynSequencerCore, materialized_cache: VecDeque<Rendezvous>) -> Self {
//         Self {
//             inner,
//             materialized_cache,
//         }
//     }
//
//     pub fn next(&mut self, epoch: DateTime<Utc>) -> Rendezvous {
//         if let Some(r) = self.materialized_cache.pop_front() {
//             return r;
//         }
//         self.materialize(epoch);
//         self.materialized_cache.pop_front().unwrap()
//     }
//     pub fn peek(&mut self, epoch: DateTime<Utc>) -> Rendezvous {
//         if let Some(r) = self.materialized_cache.front() {
//             return r.clone()
//         }
//         self.materialize(epoch);
//         self.materialized_cache.front().unwrap().clone()
//     }
//     fn materialize(&mut self, epoch: DateTime<Utc>) {
//         let rz = self.inner.materialize_rendezvous_sequence(epoch, 1);
//         // PANICS: internal invariant, should never happen.
//         assert!(
//             !rz.is_empty(),
//             "invariant broken: sequencer materialized empty rendezvous sequencer"
//         );
//         self.materialized_cache.extend(rz);
//     }
// }
