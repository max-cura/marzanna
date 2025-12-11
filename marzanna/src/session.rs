use crate::Rendezvous;
use crate::sequencer::DomainSequencer;
use crate::sequencer::DynDomainSequencer;
use crate::sequencer::DynTimeSequencer;
use crate::sequencer::TimeSequencer;
use crate::sequencer::Timepoint;
use aes_gcm_siv::Aes256GcmSiv;
use aes_gcm_siv::Key;
use aes_gcm_siv::KeyInit as _;
use chrono::{DateTime, Utc};
use hkdf::Hkdf;
use hostaddr::Buffer;
use hostaddr::Domain;
use rand::Rng as _;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

// #[derive(Debug, Serialize, Deserialize)]
// struct SessionConfig {
//     epoch: DateTime<Utc>,
//     resolver: SocketAddr,
//     rtt: Duration,

//     sequencer: Sequencer,

//     root_key: String,
//     #[serde(with = "crate::serde_chacha")]
//     cell_stream_cipher: ChaCha20Rng,

//     ttl_cache: HashMap<String, Ttl>,
// }
// impl From<Session> for SessionConfig {
//     fn from(value: Session) -> Self {
//         let Session {
//             epoch,
//             sequencer,
//             root_key,
//             cell_stream_cipher,
//             resolver,
//             rtt,
//             ttl_cache,
//         } = value;
//         let root_key = base64::prelude::BASE64_STANDARD.encode(&root_key);
//         Self {
//             epoch,
//             sequencer,
//             root_key,
//             cell_stream_cipher,
//             resolver,
//             rtt,
//             ttl_cache,
//         }
//     }
// }
// impl TryFrom<SessionConfig> for Session {
//     type Error = eyre::Report;

//     fn try_from(value: SessionConfig) -> Result<Self, Self::Error> {
//         let SessionConfig {
//             epoch,
//             sequencer,
//             root_key,
//             cell_stream_cipher,
//             resolver,
//             rtt,
//             ttl_cache,
//         } = value;
//         let root_key = base64::prelude::BASE64_STANDARD
//             .decode(&root_key)
//             .wrap_err("failed to decode shared key")?;
//         Ok(Self {
//             epoch,
//             sequencer,
//             root_key,
//             cell_stream_cipher,
//             resolver,
//             rtt,
//             ttl_cache,
//         })
//     }
// }

// #[derive(Debug, Clone, Serialize, Deserialize)]
// #[serde(into = "SessionConfig", try_from = "SessionConfig")]
// pub struct Session {
//     epoch: DateTime<Utc>,
//     sequencer: Sequencer,
//     root_key: Vec<u8>,
//     cell_stream_cipher: ChaCha20Rng,
//     resolver: SocketAddr,
//     rtt: Duration,
//     ttl_cache: HashMap<String, Ttl>,
// }

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Party {
    Alice,
    Bob,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedTimeSequencer {
    inner: DynTimeSequencer,
    cache: VecDeque<Timepoint>,
}
impl CachedTimeSequencer {
    pub fn new(inner: DynTimeSequencer) -> Self {
        Self {
            inner,
            cache: Default::default(),
        }
    }
    pub fn peek_time(&mut self, epoch: DateTime<Utc>) -> Timepoint {
        if let Some(f) = self.cache.front() {
            f.clone()
        } else {
            self.cache.push_back(self.inner.next_time(epoch));
            self.cache.front().expect("cache is nonempty").clone()
        }
    }
    pub fn commit_materialized(&mut self) -> Option<Timepoint> {
        self.cache.pop_front()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionParams {
    epoch: DateTime<Utc>,
    resolver: SocketAddr,
    rtt: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCrypto {
    #[serde(with = "hex::serde")]
    aead_root_key: [u8; 32],
    #[serde(with = "crate::serde_chacha")]
    cell_cipher_stream: ChaCha20Rng,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ABTuple<T>(T, T);
impl<T> ABTuple<T> {
    pub fn get(&self, party: Party) -> &T {
        match party {
            Party::Alice => &self.0,
            Party::Bob => &self.1,
        }
    }
    pub fn get_mut(&mut self, party: Party) -> &mut T {
        match party {
            Party::Alice => &mut self.0,
            Party::Bob => &mut self.1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct RendezvousIdxGen(usize);
impl RendezvousIdxGen {
    pub fn peek_idx(&self) -> usize {
        self.0
    }
    pub fn next(&mut self) {
        self.0 += 1;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    params: SessionParams,
    crypto: SessionCrypto,

    time_sequencers: ABTuple<CachedTimeSequencer>,
    rendezvous_indices: ABTuple<RendezvousIdxGen>,
    shared_domain_sequencer: DynDomainSequencer,
    cached_domains: ABTuple<VecDeque<Domain<Buffer>>>,
    domain_cache: HashMap<String, u64>,
}
impl Session {
    pub fn fresh(
        epoch: DateTime<Utc>,
        resolver: SocketAddr,
        rtt: Duration,
        build_time_sequencer: impl Fn() -> DynTimeSequencer,
        shared_domain_sequencer: DynDomainSequencer,
    ) -> Self {
        let aead_root_key: [u8; 32] = ChaCha20Rng::from_os_rng().random();
        Self {
            params: SessionParams {
                epoch,
                resolver,
                rtt,
            },
            crypto: SessionCrypto {
                aead_root_key,
                cell_cipher_stream: ChaCha20Rng::from_os_rng(),
            },
            time_sequencers: ABTuple(
                CachedTimeSequencer::new(build_time_sequencer()),
                CachedTimeSequencer::new(build_time_sequencer()),
            ),
            rendezvous_indices: ABTuple::default(),
            shared_domain_sequencer,
            cached_domains: ABTuple::default(),
            domain_cache: Default::default(),
        }
    }
}

impl Session {
    pub fn resolver(&self) -> SocketAddr {
        self.params.resolver
    }
    pub fn rtt(&self) -> Duration {
        self.params.rtt
    }
    pub fn derive_per_cell_key(&self, cell_id: u64) -> Aes256GcmSiv {
        let mut okm = [0u8; 42];
        let salt: [u8; 16] = (cell_id as u128).to_le_bytes();
        let derived = Hkdf::<Sha256>::new(Some(&salt[..]), &self.crypto.aead_root_key);
        derived
            .expand(b"derived", &mut okm)
            .expect("42 is a valid length for Sha256 to output");
        let mut key_bits = [0u8; 32];
        key_bits[..32].copy_from_slice(&okm[..32]);
        let key = Key::<Aes256GcmSiv>::from(key_bits);
        Aes256GcmSiv::new(&key)
    }
    pub fn cell_stream_cipher_mut(&mut self) -> &mut ChaCha20Rng {
        &mut self.crypto.cell_cipher_stream
    }
}

impl Session {
    pub fn commit_materialized_rendezvous(&mut self, party: Party) {
        assert!(
            self.time_sequencers
                .get_mut(party)
                .commit_materialized()
                .is_some()
        );
        assert!(self.cached_domains.get_mut(party).pop_front().is_some());
        self.rendezvous_indices.get_mut(party).next();
    }

    pub fn peek_next_rendezvous(&mut self, party: Party) -> Rendezvous {
        let timepoint = self
            .time_sequencers
            .get_mut(party)
            .peek_time(self.params.epoch);
        let idx = self.rendezvous_indices.get(party).peek_idx();

        let host = self.peek_next_domain(party);

        Rendezvous {
            idx,
            host,
            timepoint,
        }
    }

    fn peek_next_domain(&mut self, party: Party) -> Domain<Buffer> {
        // key requirement: we always materialize domains in pairs

        if let Some(domain) = self.cached_domains.get(party).front() {
            domain.clone()
        } else {
            loop {
                let (alice, bob) = self.materialize_domain_pair();
                let alice_ok = self.opportunistic_insert(Party::Alice, alice.0, alice.1);
                let bob_ok = self.opportunistic_insert(Party::Bob, bob.0, bob.1);
                let ok = match party {
                    Party::Alice => alice_ok,
                    Party::Bob => bob_ok,
                };
                if ok {
                    break;
                }
            }

            self.cached_domains
                .get(party)
                .front()
                .expect("cached_domains.(party) should be nonempty")
                .clone()
        }
    }

    fn opportunistic_insert(
        &mut self,
        party: Party,
        domain: Domain<Buffer>,
        initial_ttl: u64,
    ) -> bool {
        let ttl_ok = match self
            .domain_cache
            .entry(domain.into_inner().as_str().to_string())
        {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                if *occupied.get() == 0 {
                    occupied.insert(initial_ttl);
                    true
                } else {
                    *occupied.get_mut() -= 1;
                    false
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(initial_ttl);
                true
            }
        };
        if ttl_ok {
            self.cached_domains.get_mut(party).push_back(domain);
        }
        ttl_ok
    }

    fn materialize_domain_pair(&mut self) -> ((Domain<Buffer>, u64), (Domain<Buffer>, u64)) {
        let alice = self.shared_domain_sequencer.next_domain();
        let bob = self.shared_domain_sequencer.next_domain();
        (alice, bob)
    }
}

// impl Session {
//     pub fn fresh(
//         epoch: DateTime<Utc>,
//         sequencer: Sequencer,
//         root_key: Vec<u8>,
//         resolver: SocketAddr,
//         rtt: Duration,
//     ) -> Self {
//         Self {
//             epoch,
//             sequencer,
//             root_key,
//             cell_stream_cipher: ChaCha20Rng::from_os_rng(),
//             resolver,
//             ttl_cache: HashMap::new(),
//             rtt,
//         }
//     }

//     pub fn peek_next_rendezvous(&mut self, party: Party) -> Rendezvous {
//         // Next time in the sequence
//         let [can_write_at, write_by, can_read_at, read_by] = match party {
//             Party::Alice => self.sequencer.peek_time(self.epoch),
//             Party::Bob => self.sequencer_bob.peek_time(self.epoch),
//         };
//         // Get the next idx
//         let idx = match party {
//             Party::Alice => self.sequencer.idx(),
//             Party::Bob => self.sequencer_bob.idx(),
//         };
//         // Domain selection: it's very important that this be stable
//         // This is why we use the integral TTL system; it's very stable and doesn't need to care
//         // about clock drift. However, it is fundamentally probabilistic, which may or may not be an
//         // issue later down the line.
//         let (host, cache_ttl) = loop {
//             let (host, cache_ttl) = match party {
//                 Party::Alice => self.sequencer.peek_domain(),
//                 Party::Bob => self.sequencer_bob.peek_domain(),
//             };
//             // tracing::trace!("try {host}:{cache_ttl}");
//             let rdzv_domain_str = host.as_ref().into_inner().as_str();
//             if let Some(ttl) = self.ttl_cache.get_mut(rdzv_domain_str) {
//                 match ttl {
//                     Ttl::Time(time) => {
//                         if *time == 0 {
//                             // tracing::trace!("ttl -> 0 -> ({host})");
//                             *ttl = Ttl::Placeholder;
//                             break (host, cache_ttl);
//                         } else {
//                             // tracing::trace!("ttl -> {} -> pop", *time);
//                             match party {
//                                 Party::Alice => self.sequencer.pop_domain(),
//                                 Party::Bob => self.sequencer_bob.pop_domain(),
//                             };
//                         }
//                         *time -= 1;
//                     }
//                     Ttl::Placeholder => {
//                         // tracing::trace!("placeholder -> pop");
//                         // already holding out
//                         // match party {
//                         //     Party::Alice => self.sequencer_alice.pop_domain(),
//                         //     Party::Bob => self.sequencer_bob.pop_domain(),
//                         // };
//                         break (host, cache_ttl);
//                     }
//                 }
//             } else {
//                 // tracing::trace!("{host} not in ttl cache, putting placeholder");
//                 self.ttl_cache
//                     .insert(rdzv_domain_str.to_string(), Ttl::Placeholder);
//                 break (host, cache_ttl);
//             }
//         };
//         // tracing::trace!("accept {host}:{cache_ttl}");
//         Rendezvous {
//             idx,
//             host,
//             can_write_at,
//             write_by,
//             can_read_at,
//             read_by,
//             cache_ttl,
//         }
//     }
//     pub fn commit_rendezvous(&mut self, party: Party) {
//         match party {
//             Party::Alice => self.sequencer.next_idx(),
//             Party::Bob => self.sequencer_bob.next_idx(),
//         }
//         let rz_query = match party {
//             Party::Alice => self.sequencer.pop_domain(),
//             Party::Bob => self.sequencer_bob.pop_domain(),
//         };
//         match party {
//             Party::Alice => self.sequencer.pop_time(),
//             Party::Bob => self.sequencer_bob.pop_time(),
//         }
//         let mr = self
//             .ttl_cache
//             .get_mut(rz_query.0.as_ref().into_inner().as_str())
//             .expect("uncommited domain must be in cache");
//         *mr = Ttl::Time(rz_query.1);
//         // self.ttl_cache.get_mut()
//         // let rz = match party {
//         //     Party::Alice => self.sequencer_alice.next(self.epoch),
//         //     Party::Bob => self.sequencer_bob.next(self.epoch),
//         // };
//         // if let Some(ttl_sec) = ttl_sec {
//         //     let s = rz.domain().into_inner().as_str().to_string();
//         //     self.ttl_cache.insert(s, Utc::now() + ttl_sec)
//         // }
//     }
//     pub fn cell_stream_cipher_mut(&mut self) -> &mut ChaCha20Rng {
//         &mut self.cell_stream_cipher
//     }
//     pub fn resolver(&self) -> SocketAddr {
//         self.resolver
//     }
//     pub fn shared_key(&self) -> &[u8] {
//         &self.root_key
//     }
//     pub fn rtt(&self) -> Duration {
//         self.rtt
//     }
// }
