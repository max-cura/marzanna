#![feature(iter_array_chunks)]
#![feature(vec_deque_truncate_front)]
extern crate core;

mod open;
mod init;

use clap::{Parser, Subcommand};

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() -> eyre::Result<()> {
    let args = <Opts as Parser>::parse();
    color_eyre::install()?;

    match args.invocation {
        Invocation::Init(opts) => {init::invoke(opts).await}
        Invocation::Open(opts) => {open::invoke(opts).await}
    }
}

#[derive(Debug, Parser)]
#[command(version, about, propagate_version = true)]
struct Opts {
    #[command(subcommand)]
    invocation: Invocation,
}

#[derive(Debug, Subcommand)]
enum Invocation {
    Init(init::Opts),
    Open(open::Opts),
}
