use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod client;
mod server;
mod session;
mod util {
    pub fn dump_bits(bits: &[bool]) {
        for (idx32, chunk) in bits.chunks(32).enumerate() {
            print!("{idx32:>4}\t|");
            for (i, b) in chunk.iter().enumerate() {
                if i.is_multiple_of(8) {
                    print!("  ");
                }
                print!("{}", if *b { "1" } else { "0" });
            }
            println!()
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() -> eyre::Result<()> {
    let args = <Opts as Parser>::parse();
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    match args.invocation {
        Invocation::Session(opts) => session::invoke(opts).await,
        Invocation::Client(opts) => client::invoke(opts).await,
        Invocation::Server(opts) => server::invoke(opts).await,
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
    Session(session::Opts),
    Client(client::Opts),
    Server(server::Opts),
}
