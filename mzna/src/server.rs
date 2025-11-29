use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct Opts {
    #[command(subcommand)]
    invocation: Invocation,
}

#[derive(Debug, Subcommand)]
enum Invocation {
    Bind(bind::Opts),
}

mod bind {
    use clap::Args;
    use hickory_client::{
        client::{Client, ClientHandle as _},
        proto::{
            rr::{DNSClass, RecordType},
            runtime::TokioRuntimeProvider,
            udp::UdpClientStream,
        },
    };
    use hickory_resolver::Name;
    use marzanna::{Session, driver::DriverError};
    use std::{path::PathBuf, str::FromStr as _};
    use tokio::sync::{
        mpsc::{self, OwnedPermit},
        oneshot,
    };

    #[derive(Debug, Args)]
    pub struct Opts {
        #[arg(short = 's')]
        session: PathBuf,
        // TODO: adaptive retries
        #[arg(short = 'r')]
        retries: u32,
    }

    pub async fn invoke(opts: Opts) -> eyre::Result<()> {
        let session: Session = serde_json::from_str(&std::fs::read_to_string(opts.session)?)?;

        let (ev_tx, mut ev_rx) = mpsc::channel(128);
        let (term_tx, term_rx) = oneshot::channel();

        let resolver = session.resolver();

        let jh = marzanna::driver::drive(
            session,
            |rendezvous| (rendezvous.spec_time(), rendezvous.server_read_time()),
            move |rendezvous, permit: OwnedPermit<Result<(usize, bool), DriverError>>| {
                let _ = tokio::spawn(async move {
                    tracing::trace!("event for idx={}", rendezvous.idx());
                    let conn =
                        UdpClientStream::builder(resolver, TokioRuntimeProvider::default()).build();
                    let (mut client, bg) = Client::connect(conn).await.unwrap();
                    tokio::spawn(bg);

                    let domain = rendezvous.domain();
                    let name = Name::from_str(domain.as_inner().as_str())
                        .expect("rendezvous domain is valid");
                    tracing::debug!("sending DNS A request for {}", domain);
                    let response = client
                        .query(name, DNSClass::IN, RecordType::A)
                        .await
                        .unwrap();
                    let soa = response.soa().unwrap().to_owned();

                    // tracing::debug!("{soa:?}");
                    let bit = soa.ttl() < soa.data().minimum();
                    permit.send(Ok((rendezvous.idx(), bit)));
                });
            },
            ev_tx,
            term_rx,
        );

        let mut ctrl_c = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

        loop {
            let ev = tokio::select! {
                _ = ctrl_c.recv() => {
                    println!("Received ^C, exiting");
                    break
                }
                ev = ev_rx.recv() => {
                    ev
                }
            };
            tracing::debug!("received event {ev:?}");
        }

        let _ = term_tx.send(());

        let (_session, _unfinished_rendezvous, _res) = jh.await?;

        Ok(())
    }
}

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    match opts.invocation {
        Invocation::Bind(opts) => bind::invoke(opts).await,
    }
}
