//! `libmarzanna` provides the functionality of marzanna as a library.
//!
//! This crate doesn't handle any of the actual networking that happens, only the protocol level
//! state and mechanisms.
#![feature(slice_as_array)]

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use hostaddr::{Buffer, Domain};
use serde::{Deserialize, Serialize};

mod bits;
pub mod driver;
pub mod sequencer;
pub mod codec {
    use bytes::{Bytes, BytesMut};
    use enum_dispatch::enum_dispatch;
    use serde::{Deserialize, Serialize};

    mod simple7x1;
    pub use simple7x1::Simple7x1;

    #[enum_dispatch]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "name")]
    pub enum DynCodec {
        Simple7x1,
    }

    #[enum_dispatch(DynCodec)]
    pub trait Codec {
        fn required_bits(&self, msg: &Bytes) -> usize;
        fn encode(&mut self, msg: &Bytes, buf: &mut [bool]);
        fn decode(&mut self, msg: &[bool], buf: &mut BytesMut);
    }
}
mod serde_chacha;

use codec::DynCodec;
use sequencer::{DynSequencerCore, SequencerCore as _};
use std::net::SocketAddr;

#[derive(Debug, Serialize, Deserialize)]
struct SessionConfig {
    epoch: DateTime<Utc>,
    sequencer: DynSequencerCore,
    codec: DynCodec,
    sequencer_cache: Vec<Rendezvous>,
    resolver: SocketAddr,
}
impl From<Session> for SessionConfig {
    fn from(value: Session) -> Self {
        let Session {
            epoch,
            sequencer,
            codec,
            resolver,
        } = value;
        let Sequencer {
            inner,
            materialized_cache,
        } = sequencer;
        Self {
            epoch,
            sequencer: inner,
            codec,
            sequencer_cache: materialized_cache.into(),
            resolver,
        }
    }
}
impl TryFrom<SessionConfig> for Session {
    type Error = eyre::Report;

    fn try_from(value: SessionConfig) -> Result<Self, Self::Error> {
        let SessionConfig {
            epoch,
            sequencer,
            codec,
            sequencer_cache,
            resolver,
        } = value;
        let sequencer = Sequencer::new(sequencer, VecDeque::from(sequencer_cache));
        Ok(Self {
            epoch,
            sequencer,
            codec,
            resolver,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "SessionConfig", try_from = "SessionConfig")]
pub struct Session {
    epoch: DateTime<Utc>,
    sequencer: Sequencer,
    codec: DynCodec,
    resolver: SocketAddr,
}
impl Session {
    pub fn fresh(
        epoch: DateTime<Utc>,
        sequencer: DynSequencerCore,
        codec: DynCodec,
        resolver: SocketAddr,
    ) -> Self {
        Self {
            epoch,
            sequencer: Sequencer::new(sequencer, VecDeque::new()),
            codec,
            resolver,
        }
    }
    pub fn next_rendezvous(&mut self) -> Rendezvous {
        self.sequencer.next(self.epoch)
    }
    pub fn codec_mut(&mut self) -> &mut DynCodec {
        &mut self.codec
    }
    pub fn resolver(&self) -> SocketAddr {
        self.resolver
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Rendezvous {
    idx: usize,
    host: Domain<Buffer>,
    time: DateTime<Utc>,
    /// Margin before and after the rendezvous when the client/server should write/read,
    /// respectively.
    abs_delta: chrono::TimeDelta,
}
impl std::fmt::Debug for Rendezvous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rendezvous")
            .field("idx", &self.idx)
            .field("host", &format!("{}", self.host))
            .field("time", &self.time)
            .field("abs_delta", &self.abs_delta)
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
    pub fn server_read_time(&self) -> DateTime<Utc> {
        self.time + self.abs_delta
    }
    pub fn client_write_time(&self) -> DateTime<Utc> {
        self.time - self.abs_delta
    }
    pub fn spec_time(&self) -> DateTime<Utc> {
        self.time
    }
}

#[derive(Debug, Clone)]
pub struct Sequencer {
    inner: DynSequencerCore,
    materialized_cache: VecDeque<Rendezvous>,
}
impl Sequencer {
    pub(crate) fn new(inner: DynSequencerCore, materialized_cache: VecDeque<Rendezvous>) -> Self {
        Self {
            inner,
            materialized_cache,
        }
    }

    pub fn next(&mut self, epoch: DateTime<Utc>) -> Rendezvous {
        if let Some(r) = self.materialized_cache.pop_front() {
            return r;
        }
        let rz = self.inner.materialize_rendezvous_sequence(epoch, 1);
        // PANICS: internal invariant, should never happen.
        assert!(
            !rz.is_empty(),
            "invariant broken: sequencer materialized empty rendezvous sequencer"
        );
        self.materialized_cache.extend(rz);
        self.materialized_cache.pop_front().unwrap()
    }
}
