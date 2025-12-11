use crate::sequencer::{DnsAlphabet, DomainSequencer};
use hostaddr::{Buffer, Domain};
use rand::{Rng as _, SeedableRng as _};
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};

//=================================================================================================
// UNIFORM NEGATIVE SINGLE LABEL
//=================================================================================================

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
    fn next_domain(&mut self) -> (Domain<Buffer>, u64) {
        let label: Vec<u8> = (&mut self.rng)
            .sample_iter(DnsAlphabet)
            .take(self.len)
            .collect();
        let host = Domain::try_from_ascii_str(&String::from_iter([
            String::from_utf8(label).expect("DnsAlphabet produces only valid ASCII"),
            self.upper.clone(),
        ]))
        .expect("Generated domain should be correct by construction")
        .into();
        (host, 0)
    }
}

//=================================================================================================
// WEIGHTED LIST
//=================================================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedListFile {
    #[serde(with = "crate::serde_chacha")]
    rng: ChaCha20Rng,
    site_list_file: PathBuf,
    bph: u64,
}
impl TryFrom<WeightedListFile> for WeightedList {
    type Error = eyre::Report;
    fn try_from(value: WeightedListFile) -> Result<Self, Self::Error> {
        let WeightedListFile {
            rng,
            site_list_file,
            bph,
        } = value;
        let f = std::fs::read_to_string(&site_list_file)?;
        let v: Vec<_> = f.lines().collect();
        Ok(WeightedList::from_ranking_list(
            rng,
            site_list_file,
            &v,
            bph,
        ))
    }
}
impl Into<WeightedListFile> for WeightedList {
    fn into(self) -> WeightedListFile {
        let Self {
            rng,
            site_list_file,
            site_list: _,
            total: _,
            bph,
        } = self;
        WeightedListFile {
            rng,
            site_list_file,
            bph,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "WeightedListFile", into = "WeightedListFile")]
pub struct WeightedList {
    rng: ChaCha20Rng,
    site_list_file: PathBuf,
    site_list: BTreeMap<u64, (String, u64)>,
    total: u64,
    bph: u64,
}
// Derived empirically based on data from Wolfram Alpha
// https://www.wolframalpha.com/input?i2d=true&i=top+100+websites+by+daily+visits
// Retrieved 1 Dec 2025
fn approximate_weight(_rank: u64) -> u64 {
    // // \approx 24 exp[-0.725(2)^-0.33x + 10x^-0.169 - 9.4]
    // let gv_day = 24f64  * (-0.725f64 * 2f64.powf(-0.33f64 * rank as f64) + 10.146f64 * (rank as f64).powf(-0.169f64) - 9.46f64).exp();
    // // g(1) = 26.77 ~= 2.4 Gv/day
    // let kv_day = gv_day * 100_00.0f64; // 10kv/day
    // // dbg!(rank, gv_day, kv_day);
    // kv_day.round() as u64
    1
}
impl WeightedList {
    pub fn from_ranking_list(
        rng: ChaCha20Rng,
        site_list_file: PathBuf,
        sites: &[&str],
        bph: u64,
    ) -> Self {
        let mut accum = 0;
        let mut site_list = BTreeMap::new();
        for (rank, site) in sites.into_iter().enumerate().rev() {
            let rank = rank + 1;
            let wgt = approximate_weight(rank as u64);
            // println!("{accum}<- + {wgt}");
            site_list.insert(accum, (site.to_string(), wgt));
            accum += wgt;
        }
        Self {
            rng,
            site_list_file,
            site_list,
            total: accum,
            bph,
        }
    }
    pub fn fresh_from_ranking_list(site_list_file: impl AsRef<Path>, bph: u64) -> Self {
        let site_list_file = site_list_file.as_ref();
        let f = std::fs::read_to_string(site_list_file).unwrap();
        let v: Vec<_> = f.lines().collect();
        Self::from_ranking_list(
            ChaCha20Rng::from_os_rng(),
            site_list_file.to_path_buf(),
            &v,
            bph,
        )
    }
}
impl DomainSequencer for WeightedList {
    fn next_domain(&mut self) -> (Domain<Buffer>, u64) {
        let v = self.rng.random_range(0..self.total);
        let db = self
            .site_list
            .upper_bound(Bound::Included(&v))
            .peek_prev()
            .unwrap()
            .1;
        let domain = Domain::try_from_ascii_str(db.0.as_str())
            .map(|x| x.into())
            .unwrap_or_else(|_| Domain::try_from_ascii_str("google.com").unwrap().into());
        // expected hits = (weight / total) * bits/hour * expected cache life
        let assumed_dns_ttl = 3600;
        let cache_ttl =
            { (db.1 as f64) / (self.total as f64) * self.bph as f64 * assumed_dns_ttl as f64 }
                as u64;
        (domain, cache_ttl)
    }
}
