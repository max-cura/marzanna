use crate::codec::DynCodec;
// use crate::sequencer::DynSequencerCore;
use crate::{Rendezvous, Sequencer};
use base64::Engine;
use chrono::{DateTime, Utc};
use eyre::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize)]
struct SessionConfig {
    epoch: DateTime<Utc>,
    resolver: SocketAddr,
    rtt: Duration,

    sequencer_alice: Sequencer,
    sequencer_bob: Sequencer,
    codec: DynCodec,

    shared_key: String,

    ttl_cache: HashMap<String, Ttl>,
}
impl From<Session> for SessionConfig {
    fn from(value: Session) -> Self {
        let Session {
            epoch,
            sequencer_alice,
            sequencer_bob,
            codec,
            shared_key,
            resolver,
            rtt,
            ttl_cache,
        } = value;
        let shared_key = base64::prelude::BASE64_STANDARD.encode(&shared_key);
        Self {
            epoch,
            sequencer_alice,
            sequencer_bob,
            codec,
            shared_key,
            resolver,
            rtt,
            ttl_cache,
        }
    }
}
impl TryFrom<SessionConfig> for Session {
    type Error = eyre::Report;

    fn try_from(value: SessionConfig) -> Result<Self, Self::Error> {
        let SessionConfig {
            epoch,
            sequencer_alice,
            sequencer_bob,
            codec,
            shared_key,
            resolver,
            rtt,
            ttl_cache,
        } = value;
        let shared_key = base64::prelude::BASE64_STANDARD
            .decode(&shared_key)
            .wrap_err("failed to decode shared key")?;
        Ok(Self {
            epoch,
            sequencer_alice,
            sequencer_bob,
            codec,
            shared_key,
            resolver,
            rtt,
            ttl_cache,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "SessionConfig", try_from = "SessionConfig")]
pub struct Session {
    epoch: DateTime<Utc>,
    sequencer_alice: Sequencer,
    sequencer_bob: Sequencer,
    codec: DynCodec,
    shared_key: Vec<u8>,
    resolver: SocketAddr,
    rtt: Duration,
    ttl_cache: HashMap<String, Ttl>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Ttl {
    Time(u64),
    Placeholder,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Party {
    Alice,
    Bob,
}

impl Session {
    pub fn fresh(
        epoch: DateTime<Utc>,
        sequencer_alice: Sequencer,
        sequencer_bob: Sequencer,
        codec: DynCodec,
        shared_key: Vec<u8>,
        resolver: SocketAddr,
        rtt: Duration,
    ) -> Self {
        Self {
            epoch,
            sequencer_alice,
            sequencer_bob,
            codec,
            shared_key,
            resolver,
            ttl_cache: HashMap::new(),
            rtt,
        }
    }
    pub fn peek_next_rendezvous(&mut self, party: Party) -> Rendezvous {
        // Next time in the sequence
        let [can_write_at, write_by, can_read_at, read_by] = match party {
            Party::Alice => self.sequencer_alice.peek_time(self.epoch),
            Party::Bob => self.sequencer_bob.peek_time(self.epoch),
        };
        // Get the next idx
        let idx = match party {
            Party::Alice => self.sequencer_alice.idx(),
            Party::Bob => self.sequencer_bob.idx(),
        };
        // Domain selection: it's very important that this be stable
        // This is why we use the integral TTL system; it's very stable and doesn't need to care
        // about clock drift. However, it is fundamentally probabilistic, which may or may not be an
        // issue later down the line.
        let (host, cache_ttl) = loop {
            let (host, cache_ttl) = match party {
                Party::Alice => self.sequencer_alice.peek_domain(),
                Party::Bob => self.sequencer_bob.peek_domain(),
            };
            // tracing::trace!("try {host}:{cache_ttl}");
            let rdzv_domain_str = host.as_ref().into_inner().as_str();
            if let Some(ttl) = self.ttl_cache.get_mut(rdzv_domain_str) {
                match ttl {
                    Ttl::Time(time) => {
                        if *time == 0 {
                            // tracing::trace!("ttl -> 0 -> ({host})");
                            *ttl = Ttl::Placeholder;
                            break (host, cache_ttl);
                        } else {
                            // tracing::trace!("ttl -> {} -> pop", *time);
                            match party {
                                Party::Alice => self.sequencer_alice.pop_domain(),
                                Party::Bob => self.sequencer_bob.pop_domain(),
                            };
                        }
                        *time -= 1;
                    }
                    Ttl::Placeholder => {
                        // tracing::trace!("placeholder -> pop");
                        // already holding out
                        // match party {
                        //     Party::Alice => self.sequencer_alice.pop_domain(),
                        //     Party::Bob => self.sequencer_bob.pop_domain(),
                        // };
                        break (host, cache_ttl);
                    }
                }
            } else {
                // tracing::trace!("{host} not in ttl cache, putting placeholder");
                self.ttl_cache
                    .insert(rdzv_domain_str.to_string(), Ttl::Placeholder);
                break (host, cache_ttl);
            }
        };
        // tracing::trace!("accept {host}:{cache_ttl}");
        Rendezvous {
            idx,
            host,
            can_write_at,
            write_by,
            can_read_at,
            read_by,
            cache_ttl,
        }
    }
    pub fn commit_rendezvous(&mut self, party: Party) {
        match party {
            Party::Alice => self.sequencer_alice.next_idx(),
            Party::Bob => self.sequencer_bob.next_idx(),
        }
        let rz_query = match party {
            Party::Alice => self.sequencer_alice.pop_domain(),
            Party::Bob => self.sequencer_bob.pop_domain(),
        };
        match party {
            Party::Alice => self.sequencer_alice.pop_time(),
            Party::Bob => self.sequencer_bob.pop_time(),
        }
        let mr = self
            .ttl_cache
            .get_mut(rz_query.0.as_ref().into_inner().as_str())
            .expect("uncommited domain must be in cache");
        *mr = Ttl::Time(rz_query.1);
        // self.ttl_cache.get_mut()
        // let rz = match party {
        //     Party::Alice => self.sequencer_alice.next(self.epoch),
        //     Party::Bob => self.sequencer_bob.next(self.epoch),
        // };
        // if let Some(ttl_sec) = ttl_sec {
        //     let s = rz.domain().into_inner().as_str().to_string();
        //     self.ttl_cache.insert(s, Utc::now() + ttl_sec)
        // }
    }
    pub fn codec_mut(&mut self) -> &mut DynCodec {
        &mut self.codec
    }
    pub fn resolver(&self) -> SocketAddr {
        self.resolver
    }
    pub fn shared_key(&self) -> &[u8] {
        &self.shared_key
    }
    pub fn rtt(&self) -> Duration {
        self.rtt
    }
}
