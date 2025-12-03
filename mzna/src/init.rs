use chrono::Utc;
use clap::Args;
use eyre::Context;
use marzanna::Sequencer;
#[allow(unused_imports)]
use marzanna::codec::{DynCodec, Interleaved5x1, Interleaved7x1, Interleaved3x1, Simple3x1};
use marzanna::sequencer::ortho::domain::WeightedList;
use marzanna::sequencer::ortho::time::UniformOffset;
use marzanna::sequencer::ortho::{DynDomainSequencer, DynTimeSequencer};
use marzanna::session::Session;
use rand::{Rng, SeedableRng};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Args)]
pub struct Opts {
    #[arg(short = 's')]
    session_file: Option<PathBuf>,

    #[arg(short = 'r')]
    resolver: SocketAddr,

    #[arg(long)]
    rtt: String,

    #[arg(long)]
    subsec_to: u64,

    #[arg(long)]
    list: PathBuf,
}

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    fn build_sequencer(opts: &Opts) -> Sequencer {
        Sequencer::new(
            DynTimeSequencer::UniformOffset(UniformOffset::new(1, 2, 10, opts.subsec_to)),
            // DynDomainSequencer::UniformNSL(UniformNSL::new(32, ".com")),
            DynDomainSequencer::WeightedList(WeightedList::fresh_from_ranking_list(
                &opts.list, 36_000,
            )),
        )
    }

    // let codec = DynCodec::Simple7x1(Simple7x1);
    // let codec = DynCodec::Simple5x1(Simple5x1);
    // let codec = DynCodec::Simple3x1(Simple3x1);
    // Interleaved is much more resilient to network weather
    // let codec = DynCodec::Interleaved7x1(Interleaved7x1);
    let codec = DynCodec::Interleaved5x1(Interleaved5x1);

    let shared_key: [u8; 32] = rand_chacha::ChaCha20Rng::from_os_rng().random();

    let rtt = fundu::DurationParser::new()
        .parse(&opts.rtt)
        .wrap_err("failed to parse rtt")?;

    let session = Session::fresh(
        Utc::now(),
        build_sequencer(&opts),
        build_sequencer(&opts),
        codec,
        shared_key.to_vec(),
        opts.resolver,
        rtt.try_into().wrap_err("rtt out of range")?,
    );
    let session_json =
        serde_json::to_string(&session).wrap_err("failed to serialize Marzanna session")?;

    if let Some(session_file) = opts.session_file.as_ref() {
        std::fs::write(session_file, session_json)
            .wrap_err("failed to write Marzanna session file")?;
    } else {
        println!("{}", session_json);
    }

    Ok(())
}
