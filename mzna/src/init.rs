use chrono::{TimeDelta, Utc};
use clap::Args;
use eyre::Context;
#[allow(unused_imports)]
use marzanna::codec::{DynCodec, Interleaved3x1, Interleaved5x1, Interleaved7x1, Simple3x1};
use marzanna::sequencer::domain::WeightedList;
use marzanna::sequencer::time::UniformOffset;
use marzanna::sequencer::{DynDomainSequencer, DynTimeSequencer};
use marzanna::session::Session;
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

    let rtt = fundu::DurationParser::new()
        .parse(&opts.rtt)
        .wrap_err("failed to parse rtt")?;

    let session = Session::fresh(
        Utc::now() + TimeDelta::seconds(10),
        opts.resolver,
        rtt.try_into().wrap_err("rtt out of range")?,
        || DynTimeSequencer::UniformOffset(UniformOffset::new(1, 2, 10, opts.subsec_to)),
        DynDomainSequencer::WeightedList(WeightedList::fresh_from_ranking_list(&opts.list, 36_000)),
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
