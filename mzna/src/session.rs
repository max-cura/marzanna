use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct Opts {
    #[command(subcommand)]
    invocation: Invocation,
}

#[derive(Debug, Subcommand)]
enum Invocation {
    Init(init::Opts),
}

mod init {
    // use chrono::{TimeDelta, Utc};
    use clap::Args;
    // use marzanna::{
    //     Session,
    //     codec::DynCodec,
    //     sequencer::{
    //         DynSequencerCore,
    //         ortho::{
    //             DynDomainSequencer, DynTimeSequencer, Ortho, domain::UniformNSL,
    //             time::UniformOffset,
    //         },
    //     },
    // };

    #[derive(Debug, Args)]
    pub struct Opts {
        //
    }

    pub async fn invoke(_opts: Opts) -> eyre::Result<()> {
        // let session = Session::fresh(
        //     Utc::now(),
        //     DynSequencerCore::Ortho(Ortho::fresh(
        //         DynTimeSequencer::UniformOffset(UniformOffset::new(30, 31)),
        //         DynDomainSequencer::UniformNSL(UniformNSL::new(10, ".com")),
        //         TimeDelta::seconds(10),
        //     )),
        //     DynCodec::Simple7x1(marzanna::codec::Simple7x1),
        // );

        // println!("{}", serde_json::to_string_pretty(&session)?);
        // println!(
        //     "{}",
        //     serde_json::to_string_pretty(&session.next_rendezvous())?
        // );

        Ok(())
    }
}

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    match opts.invocation {
        Invocation::Init(opts) => init::invoke(opts).await,
    }
}
