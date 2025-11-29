use chrono::{DateTime, Utc};
use enum_dispatch::enum_dispatch;
use rand::distr::Distribution;
use serde::{Deserialize, Serialize};

use crate::Rendezvous;
use ortho::Ortho;

pub struct DnsAlphabet;
impl Distribution<u8> for DnsAlphabet {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> u8 {
        // range: a-z0-9\-, since a-z=A-Z since DNS is case-insensitive; size is 36
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        ALPHABET[rng.random_range(0..ALPHABET.len())]
    }
}

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DynSequencerCore {
    Ortho,
}

/// Trait for sequencer implementations. API is unstable. Should only be used by way of a
/// [`Sequencer`].
#[enum_dispatch(DynSequencerCore)]
pub trait SequencerCore: Send {
    fn materialize_rendezvous_sequence(
        &mut self,
        epoch: DateTime<Utc>,
        count_hint: usize,
    ) -> Vec<Rendezvous>;
}

pub mod ortho {
    use chrono::{DateTime, TimeDelta, Utc};
    use enum_dispatch::enum_dispatch;
    use hostaddr::{Buffer, Domain};
    use serde::{Deserialize, Serialize};

    use super::SequencerCore;
    use crate::Rendezvous;
    use domain::UniformNSL;
    use time::UniformOffset;

    #[enum_dispatch]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "name")]
    pub enum DynTimeSequencer {
        UniformOffset,
    }
    #[enum_dispatch(DynTimeSequencer)]
    pub trait TimeSequencer: Send {
        fn next_time(&mut self, epoch: DateTime<Utc>) -> DateTime<Utc>;
    }

    #[enum_dispatch]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "name")]
    pub enum DynDomainSequencer {
        UniformNSL,
    }
    #[enum_dispatch(DynDomainSequencer)]
    pub trait DomainSequencer: Send {
        fn next_domain(&mut self) -> Domain<Buffer>;
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Ortho {
        time: DynTimeSequencer,
        domain: DynDomainSequencer,
        idx: usize,
        delta: TimeDelta,
    }
    impl Ortho {
        pub fn fresh(time: DynTimeSequencer, domain: DynDomainSequencer, delta: TimeDelta) -> Self {
            Self {
                time,
                domain,
                idx: 0,
                delta,
            }
        }
    }
    impl SequencerCore for Ortho {
        fn materialize_rendezvous_sequence(
            &mut self,
            epoch: DateTime<Utc>,
            count_hint: usize,
        ) -> Vec<Rendezvous> {
            let mut col = vec![];
            for _ in 0..count_hint {
                let idx = self.idx;
                self.idx += 1;
                col.push(Rendezvous {
                    idx,
                    host: self.domain.next_domain(),
                    time: self.time.next_time(epoch),
                    abs_delta: self.delta,
                })
            }
            col
        }
    }

    pub mod time {
        use chrono::{DateTime, TimeDelta, Utc};
        use rand::{Rng as _, SeedableRng};
        use rand_chacha::ChaCha20Rng;
        use serde::{Deserialize, Serialize};

        use crate::sequencer::ortho::TimeSequencer;

        #[derive(Debug, Deserialize)]
        struct UnvalidatedUniformOffset {
            #[serde(with = "crate::serde_chacha")]
            rng: ChaCha20Rng,
            last: u64,
            min_sec: u64,
            max_sec: u64,
        }
        impl TryFrom<UnvalidatedUniformOffset> for UniformOffset {
            type Error = eyre::Report;

            fn try_from(value: UnvalidatedUniformOffset) -> Result<Self, Self::Error> {
                #[rustfmt::skip] let UnvalidatedUniformOffset { rng, last, min_sec, max_sec } = value;
                #[rustfmt::skip] let this = UniformOffset { rng, last, min_sec, max_sec };
                eyre::ensure!(this.last < (i64::MAX as u64));
                eyre::ensure!(this.min_sec < this.max_sec);
                eyre::ensure!(this.max_sec < (i64::MAX as u64));
                Ok(this)
            }
        }
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(try_from = "UnvalidatedUniformOffset")]
        pub struct UniformOffset {
            #[serde(with = "crate::serde_chacha")]
            rng: ChaCha20Rng,
            last: u64,
            min_sec: u64,
            max_sec: u64,
        }
        impl UniformOffset {
            pub fn new(min_sec: u64, max_sec: u64) -> Self {
                assert!(min_sec < max_sec);
                Self {
                    rng: ChaCha20Rng::from_os_rng(),
                    last: 0,
                    min_sec,
                    max_sec,
                }
            }
        }
        impl TimeSequencer for UniformOffset {
            fn next_time(&mut self, epoch: DateTime<Utc>) -> DateTime<Utc> {
                let sec_offset_distr = rand::distr::Uniform::new(self.min_sec, self.max_sec)
                    .expect("invalid uniform-offset configuration: min_sec>=max_sec");
                let sec_offset = self.rng.sample(sec_offset_distr);
                tracing::trace!("sec_offset={sec_offset}");
                self.last += sec_offset;
                let td_offset =
                    TimeDelta::seconds(self.last.try_into().expect("time offset out of range"));
                epoch + td_offset
            }
        }
    }

    pub mod domain {
        use hostaddr::{Buffer, Domain};
        use rand::{Rng as _, SeedableRng as _};
        use rand_chacha::ChaCha20Rng;
        use serde::{Deserialize, Serialize};

        use crate::sequencer::{DnsAlphabet, ortho::DomainSequencer};

        #[derive(Debug, Deserialize)]
        struct UnvalidatedUniformNSL {
            #[serde(with = "crate::serde_chacha")]
            rng: ChaCha20Rng,
            len: usize, // between 1 and 63
            upper: String,
        }
        impl TryFrom<UnvalidatedUniformNSL> for UniformNSL {
            type Error = eyre::Report;
            fn try_from(value: UnvalidatedUniformNSL) -> Result<Self, Self::Error> {
                #[rustfmt::skip] let UnvalidatedUniformNSL { rng, len, upper } = value;
                #[rustfmt::skip] let this = UniformNSL { rng, len, upper };
                eyre::ensure!(this.len >= 1 && this.len <= 63);
                eyre::ensure!(this.upper.len() + 1 + this.len <= 253);
                Ok(this)
            }
        }
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(try_from = "UnvalidatedUniformNSL")]
        pub struct UniformNSL {
            #[serde(with = "crate::serde_chacha")]
            rng: ChaCha20Rng,
            len: usize, // between 1 and 63
            upper: String,
        }
        impl UniformNSL {
            pub fn new(len: usize, upper: impl ToString) -> Self {
                assert!(len >= 1 && len <= 63);
                let upper = upper.to_string();
                assert!(upper.len() + 1 + len <= 253);
                assert!(
                    upper
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                );
                Self {
                    rng: ChaCha20Rng::from_os_rng(),
                    len,
                    upper,
                }
            }
        }
        impl DomainSequencer for UniformNSL {
            fn next_domain(&mut self) -> Domain<Buffer> {
                let label: Vec<u8> = (&mut self.rng)
                    .sample_iter(DnsAlphabet)
                    .take(self.len)
                    .collect();
                Domain::try_from_ascii_str(&String::from_iter([
                    String::from_utf8(label).expect("DnsAlphabet produces only valid ASCII"),
                    self.upper.clone(),
                ]))
                .expect("Generated domain should be correct by construction")
                .into()
            }
        }
    }
}
