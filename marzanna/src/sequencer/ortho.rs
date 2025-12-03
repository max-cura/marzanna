use chrono::{DateTime, Utc};
use enum_dispatch::enum_dispatch;
use hostaddr::{Buffer, Domain};
use serde::{Deserialize, Serialize};

pub mod time;
pub mod domain;

// use super::SequencerCore;
// use crate::Rendezvous;
use domain::UniformNSL;
use time::UniformOffset;
use domain::WeightedList;

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name")]
pub enum DynTimeSequencer {
    UniformOffset,
}
#[enum_dispatch(DynTimeSequencer)]
pub trait TimeSequencer: Send {
    fn next_time(&mut self, epoch: DateTime<Utc>) -> [DateTime<Utc>; 4];
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

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct Ortho {
//     time: DynTimeSequencer,
//     domain: DynDomainSequencer,
//     idx: usize,
// }
// impl Ortho {
//     pub fn fresh(time: DynTimeSequencer, domain: DynDomainSequencer) -> Self {
//         Self {
//             time,
//             domain,
//             idx: 0,
//         }
//     }
// }
// impl SequencerCore for Ortho {
//     fn materialize_rendezvous_sequence(
//         &mut self,
//         epoch: DateTime<Utc>,
//         count_hint: usize,
//     ) -> Vec<Rendezvous> {
//         let mut col = vec![];
//         for _ in 0..count_hint {
//             let idx = self.idx;
//             self.idx += 1;
//             let (can_write_at, write_by, can_read_at, read_by) = self.time.next_time(epoch);
//             // tracing::debug!(%can_write_at, %write_by, %can_read_at, %read_by);
//             col.push(Rendezvous {
//                 idx,
//                 host: self.domain.next_domain(),
//                 can_write_at,
//                 write_by,
//                 can_read_at,
//                 read_by,
//             })
//         }
//         col
//     }
// }

