use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct Opts {
    #[command(subcommand)]
    invocation: Invocation,
}

#[derive(Debug, Subcommand)]
enum Invocation {
    Send(send::Opts),
}

mod send {
    use std::{net::SocketAddr, path::PathBuf, pin::pin, str::FromStr as _, sync::Arc};

    use bytes::Bytes;
    use chrono::{TimeDelta, Utc};
    use clap::Args;
    use futures::stream::unfold;
    use hickory_client::{
        client::{Client, ClientHandle as _},
        proto::{
            rr::{DNSClass, RecordType},
            runtime::TokioRuntimeProvider,
            udp::UdpClientStream,
        },
    };
    use hickory_resolver::Name;
    use marzanna::{
        codec::{Codec, DynCodec},
        driver::DriverError,
        sequencer::{
            ortho::{
                domain::UniformNSL, time::UniformOffset, DynDomainSequencer, DynTimeSequencer,
                Ortho,
            },
            DynSequencerCore,
        },
    };
    use tokio::sync::{
        mpsc::{self, OwnedPermit},
        oneshot,
    };
    use marzanna::session::Session;

    #[derive(Debug, Args)]
    pub struct Opts {
        #[arg(short = 'd')]
        delay: String,
        #[arg(short = 's')]
        session: Option<PathBuf>,
        #[arg(short = 'v')]
        verbose: bool,
        #[arg(short = 'r')]
        resolver: SocketAddr,
        msg: String,
    }

    pub async fn invoke(opts: Opts) -> eyre::Result<()> {
        let delay = fundu::DurationParser::new().parse(&opts.delay)?;

        let mut session = Session::fresh(
            Utc::now() + TimeDelta::try_from(delay)?,
            DynSequencerCore::Ortho(Ortho::fresh(
                DynTimeSequencer::UniformOffset(UniformOffset::new(4, 12)),
                DynDomainSequencer::UniformNSL(UniformNSL::new(10, ".com")),
                TimeDelta::seconds(3),
            )),
            DynSequencerCore::Ortho(Ortho::fresh(
                DynTimeSequencer::UniformOffset(UniformOffset::new(4, 12)),
                DynDomainSequencer::UniformNSL(UniformNSL::new(10, ".com")),
                TimeDelta::seconds(3),
            )),
            DynCodec::Simple7x1(marzanna::codec::Simple7x1),
            opts.resolver,
        );

        let session_json = serde_json::to_string(&session)?;
        if let Some(session_path) = opts.session.as_ref() {
            std::fs::write(session_path, session_json)?;
        } else {
            println!("{}", session_json)
        }

        let bits = Arc::new({
            let codec = session.codec_mut();
            let msg_bytes = Bytes::copy_from_slice(opts.msg.as_bytes());
            let mut bits = vec![false; codec.required_bits(&msg_bytes)];
            codec.encode(&msg_bytes, &mut bits);
            bits
        });
        if opts.verbose {
            crate::util::dump_bits(&bits);
        }

        let (ev_tx, ev_rx) = mpsc::channel(128);
        let (term_tx, term_rx) = oneshot::channel();

        type Event = ();

        let resolver = session.resolver();
        let jh = marzanna::driver::drive(
            session,
            |rendezvous| (rendezvous.client_write_time(), rendezvous.spec_time()),
            move |rendezvous, permit: OwnedPermit<Result<Event, DriverError>>| {
                let bits = bits.clone();
                let _ = tokio::spawn(async move {
                    let bit = bits[rendezvous.idx()];
                    tracing::debug!("event for idx={}: {:?}", rendezvous.idx(), bit);

                    if bit {
                        let conn =
                            UdpClientStream::builder(resolver, TokioRuntimeProvider::default())
                                .build();
                        let (mut client, bg) = Client::connect(conn).await.unwrap();
                        tokio::spawn(bg);

                        let domain = rendezvous.domain();
                        let name = Name::from_str(domain.as_inner().as_str())
                            .expect("rendezvous domain is valid");
                        tracing::debug!("sending DNS A reqest for {}", domain);
                        let response = client
                            .query(name, DNSClass::IN, RecordType::A)
                            .await
                            .unwrap();
                        let soa = response.soa().unwrap().to_owned();

                        if soa.ttl() < soa.data().minimum() {
                            tracing::warn!("site already in cache!");
                        }
                    }
                    // TODO: couple edge cases we don't cover
                    //  1. is site already in cache (if bit=0)
                    //  2. ensuring that the domain sequence doesn't have internal collisions

                    permit.send(Ok(()));
                });
            },
            ev_tx,
            term_rx,
        );

        let ctrl_c = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

        use tokio_stream::StreamExt;
        enum Message {
            Stop,
            Recv(#[allow(unused)] Result<Event, DriverError>),
        }
        let a = unfold(ctrl_c, |mut x| async {
            x.recv().await;
            Some((Message::Stop, x))
        });
        let b = unfold(ev_rx, |mut x| async {
            Some((
                Message::Recv(x.recv().await.ok_or(DriverError::QueueClosed).flatten()),
                x,
            ))
        });
        let mut s = pin!(a.merge(b));
        while let Some(msg) = s.next().await {
            match msg {
                Message::Stop => break,
                Message::Recv(_) => {
                    continue; //
                }
            }
        }
        let _ = term_tx.send(());
        let (_session, _unfinished_rendezvous, _res) = jh.await?;

        Ok(())
    }
}

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    match opts.invocation {
        Invocation::Send(opts) => send::invoke(opts).await,
    }
}
