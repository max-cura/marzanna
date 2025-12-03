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
    subsec: u64,
    subsec_to: u64,
    min_sec: u64,
    max_sec: u64,
    delay: u64,
}
impl TryFrom<UnvalidatedUniformOffset> for UniformOffset {
    type Error = eyre::Report;

    fn try_from(value: UnvalidatedUniformOffset) -> Result<Self, Self::Error> {
        #[rustfmt::skip] let UnvalidatedUniformOffset { rng, last, subsec, subsec_to, min_sec, max_sec, delay } = value;
        #[rustfmt::skip] let this = UniformOffset { rng, last, subsec,subsec_to,min_sec, max_sec, delay };
        eyre::ensure!(this.last < (i64::MAX as u64));
        eyre::ensure!(this.min_sec < this.max_sec);
        eyre::ensure!(this.max_sec < (i64::MAX as u64));
        eyre::ensure!(this.delay < i64::MAX as u64);
        eyre::ensure!(this.subsec < this.subsec_to);
        Ok(this)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "UnvalidatedUniformOffset")]
pub struct UniformOffset {
    #[serde(with = "crate::serde_chacha")]
    rng: ChaCha20Rng,
    last: u64,
    subsec: u64,
    subsec_to: u64,
    min_sec: u64,
    max_sec: u64,
    delay: u64,
}
impl UniformOffset {
    pub fn new(min_sec: u64, max_sec: u64, delay: u64, subsec_to: u64) -> Self {
        assert!(min_sec < max_sec);
        Self {
            rng: ChaCha20Rng::from_os_rng(),
            last: 0,
            subsec: 0,
            min_sec,
            max_sec,
            delay,
            subsec_to,
        }
    }
}
impl TimeSequencer for UniformOffset {
    fn next_time(&mut self, epoch: DateTime<Utc>) -> [DateTime<Utc>; 4] {
        if self.subsec >= self.subsec_to {
            let sec_offset_distr = rand::distr::Uniform::new(self.min_sec, self.max_sec)
                .expect("invalid uniform-offset configuration: min_sec>=max_sec");
            let sec_offset = self.rng.sample(sec_offset_distr);
            // tracing::trace!("sec_offset={sec_offset}");
            self.last += sec_offset;
            self.subsec = 0;
        } else {
            self.subsec += 1;
        }
        let td_offset = TimeDelta::seconds(self.last.try_into().expect("time offset out of range"));
        let per_start = epoch + td_offset;
        [
            per_start,
            per_start + TimeDelta::seconds(1),
            per_start + TimeDelta::seconds(1 + self.delay as i64),
            per_start + TimeDelta::seconds(1 + self.delay as i64 + 1),
        ]
    }
}
