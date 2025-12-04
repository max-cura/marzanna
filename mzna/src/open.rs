use std::io::Write;
use aes_gcm_siv::aead::Aead;
use aes_gcm_siv::aead::generic_array::typenum::ToInt;
use aes_gcm_siv::{AeadCore, Aes256GcmSiv, KeyInit};
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use clap::Args;
use eyre::{Context};
use hickory_client::client::{Client, ClientHandle};
use hickory_client::proto::rr::{DNSClass, Name, RecordType};
use hickory_client::proto::runtime::TokioRuntimeProvider;
use hickory_client::proto::udp::UdpClientStream;
use hkdf::Hkdf;
use hostaddr::{Buffer, Domain};
use marzanna::Rendezvous;
use marzanna::codec::{Codec, DynCodec};
use marzanna::session::{Party, Session};
use rand::{Rng, SeedableRng};
use sha2::Sha256;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use hickory_client::proto::op::ResponseCode;
use ordinal::ToOrdinal;
use rustyline_async::{Readline, ReadlineEvent, SharedWriter};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Debug, Args)]
pub struct Opts {
    #[arg(short = 's')]
    session: PathBuf,
    #[arg(long)]
    unsafe_free_party: bool,

    #[arg(long)]
    local_socket: Option<PathBuf>,
}
impl Opts {
    fn parties(&self) -> (Party, Party) {
        if self.unsafe_free_party {
            (Party::Bob, Party::Alice)
        } else {
            (Party::Alice, Party::Bob)
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    neg_from_close: [u64; 2],
    pos_from_close: [u64; 2],
}

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    let session_file_contents =
        std::fs::read_to_string(&opts.session).wrap_err("failed to read session file")?;
    let mut session: Session =
        serde_json::from_str(&session_file_contents).wrap_err("failed to parse session file")?;

    let mut stats = Stats::default();

    let (out_msg_tx, out_msg_rx) = unbounded_channel();
    let (in_msg_tx, in_msg_rx) = unbounded_channel();
    let hdl_stdio = drive_stdio(&opts, out_msg_tx, in_msg_rx);

    let codec = session.codec_mut().clone();
    let shared_key = session.shared_key().to_vec();

    let recv_upto = session.peek_next_rendezvous(opts.parties().0).idx();

    let (hdl_session, bit_tx, bit_rx) = drive_bitstreams(&opts, session);
    let hdl_codec =
        drive_protocol(&opts, bit_tx, bit_rx, codec, shared_key, recv_upto, out_msg_rx, in_msg_tx);

    let mut session = hdl_session.await.wrap_err("bit driver panicked")?;
    let codec = hdl_codec.await.wrap_err("protocol driver panicked")?;
    let () = hdl_stdio.await.wrap_err("stdio driver panicked")?;

    *session.codec_mut() = codec;

    Ok(())
}

fn drive_stdio(opts: &Opts, msg_tx: UnboundedSender<OutgoingMsg>, mut msg_rx: UnboundedReceiver<IncomingMsg>) -> JoinHandle<()> {
    let (self_party, _other_party) = opts.parties();
    let (mut rl, mut stdout) = Readline::new("\x1b[33m>\x1b[0m ".into())
        .expect("failed to create readline");
    struct SharedWriterWrapper(SharedWriter);
    impl<'a> MakeWriter<'a> for SharedWriterWrapper {
        type Writer = SharedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.0.clone()
        }
    }
    tracing_subscriber::fmt()
        .with_writer(SharedWriterWrapper(stdout.clone()))
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = msg_rx.recv() => {
                    match msg {
                        Some(IncomingMsg::Msg(bytes)) => {
                            let _ = writeln!(stdout, "{:02x?}", hex::encode(&bytes));
                            let _ = writeln!(stdout, "{}", String::from_utf8_lossy(bytes.as_ref()));
                        }
                        Some(IncomingMsg::Miss { receiving_party,rendezvous_idx,rendezvous_time: _,arrival_time: _ }) => {
                            // tracing::warn!("missed {} {} window",
                            //     rendezvous_idx.to_ordinal_string(),
                            //     if receiving_party == self_party { "receive" } else { "send" }
                            // );
                        }
                        None => {
                            tracing::debug!("incoming message queue closed; terminating stdio driver");
                            break;
                        }
                    }
                }
                msg = rl.readline() => match msg {
                    Ok(ReadlineEvent::Line(line)) => {
                        if let Err(_) = msg_tx.send(OutgoingMsg::Msg(line.into())) {
                            tracing::warn!("failed to send message; outgoing message queue closed; terminating stdio driver");
                            break;
                        }
                    }
                    Ok(ReadlineEvent::Eof) => {
                        tracing::warn!("Received ^D, closing queues");
                        if let Err(_) = msg_tx.send(OutgoingMsg::Close) {
                            tracing::debug!("failed to send close message, outgoing message queue already closed; terminating stdio driver");
                        }
                        break;
                    }
                    Ok(ReadlineEvent::Interrupted) => {
                        tracing::warn!("Received ^C, closing queues");
                        if let Err(_) = msg_tx.send(OutgoingMsg::Close) {
                            tracing::debug!("failed to send close message, outgoing message queue already closed; terminating stdio driver");
                        }
                        break;
                    }
                    Err(e) => {
                        tracing::error!("readline failure: {e:?}");
                        break
                    }
                }
            }
        }
        rl.flush().unwrap(); // FIXME: feeling lazy
    })
}

enum OutgoingMsg {
    Msg(Bytes),
    Close,
}
enum IncomingMsg {
    Msg(Bytes),
    Miss {
        receiving_party: Party,
        rendezvous_idx: usize,
        #[allow(unused)]
        rendezvous_time: DateTime<Utc>,
        #[allow(unused)]
        arrival_time: DateTime<Utc>,
    },
}

fn drive_protocol(
    opts: &Opts,
    outgoing_tx: UnboundedSender<OutgoingBit>,
    mut incoming_rx: UnboundedReceiver<IncomingBit>,
    mut codec: DynCodec,
    shared_key: Vec<u8>,
    recv_is_up_to_idx: usize,
    mut out_msg_rx: UnboundedReceiver<OutgoingMsg>,
    in_msg_tx: UnboundedSender<IncomingMsg>,
) ->
    JoinHandle<DynCodec>
{

    let (_self_party, other_party) = opts.parties();

    enum FS {
        Initial,
        Salt,
        Salted { cipher: Aes256GcmSiv, proto: PS },
    }
    enum PS {
        Len,
        Payload(usize),
    }

    for i in 0u64..1000 {
        let _ = outgoing_tx.send(OutgoingBit::Bit(i.is_multiple_of(2)));
    }

    let handle = tokio::spawn(async move {
        let mut state = FS::Initial;

        let mut bb_upto: usize = recv_is_up_to_idx;
        let mut bit_buffer: VecDeque<bool> = VecDeque::new();
        let mut receive_ahead: BTreeMap<usize, bool> = BTreeMap::new();

        'outer: loop {
            tokio::select! {
                out_msg = out_msg_rx.recv() => {
                    if let Some(out_msg) = out_msg {
                        match out_msg {
                            OutgoingMsg::Msg(bytes) => {
                                // let bits = build_frame(&shared_key, &mut codec, bytes);
                                // for bit in bits {
                                //     if let Err(_) = outgoing_tx.send(OutgoingBit::Bit(bit)) {
                                //         tracing::debug!("outgoing bit queue closed, stopping protocol driver task");
                                //         break
                                //     }
                                // }
                            }
                            OutgoingMsg::Close => {
                                let _ = outgoing_tx.send(OutgoingBit::Close);
                                tracing::debug!("received OutgoingMsg::Close, stopping protocol driver task");
                                break
                            }
                        }
                    } else {
                        tracing::debug!("outgoing msg queue closed, stopping protocol driver task");
                        break
                    }
                }
                in_bit = incoming_rx.recv() => {
                    if let Some(in_bit) = in_bit {
                        // tracing::trace!(?in_bit);
                        let (idx, bit) = match in_bit {
                            IncomingBit::Bit(idx, bit) => (idx, bit),
                            IncomingBit::Miss { receiving_party, rendezvous_idx, rendezvous_time, arrival_time } => {
                                if let Err(_) = in_msg_tx.send(IncomingMsg::Miss {
                                    receiving_party, rendezvous_idx, rendezvous_time, arrival_time
                                }) {
                                    tracing::debug!("incoming msg queue closed, stopping protocol driver task");
                                    break 'outer
                                }
                                if receiving_party == other_party {
                                    // it's the other party's bit that was lost, so we don't want
                                    // to snatch that
                                    continue 'outer
                                }
                                (rendezvous_idx, /* arbitrary */ false)
                            }
                        };
                        if idx.is_multiple_of(2) != bit {
                            tracing::error!("BIT FLIP OBSERVED: INDEX={idx}");
                        }
                        if idx == bb_upto {
                            bit_buffer.push_back(bit);
                            bb_upto += 1;
                        } else {
                            // tracing::trace!("mismatch: bb_upto={bb_upto} idx={idx} bit={bit:?}");
                            receive_ahead.insert(idx, bit);
                        }
                        while let Some(entry) = receive_ahead.first_entry() {
                            // tracing::trace!("upto={} recv_ahead.fst=({},{:?})", bb_upto, *entry.key(), *entry.get());
                            if *entry.key() == bb_upto {
                                let (_idx, bit) = entry.remove_entry();
                                bit_buffer.push_back(bit);
                                bb_upto += 1;
                            } else {
                                break
                            }
                        }
                        // tracing::trace!("bit_buf={}",
                        //     bit_buffer.iter().map(|x| if *x { "1" } else { "0" }).collect::<String>(),
                        // );
                        bit_buffer.clear();
                        loop {
                            let bit_buffer_len = bit_buffer.len();
                            if bit_buffer.len() == bit_buffer_len {
                                break
                            }
                        }
                    } else {
                        // incoming_tx closed
                        tracing::debug!("incoming bit queue closed, stopping protocol driver task");
                        break
                    }
                }
            }
        }
        codec
    });

    handle
}

#[derive(Debug)]
enum OutgoingBit {
    Bit(bool),
    Close,
}
#[derive(Debug)]
enum IncomingBit {
    Bit(usize, bool),
    Miss {
        receiving_party: Party,
        rendezvous_idx: usize,
        rendezvous_time: DateTime<Utc>,
        arrival_time: DateTime<Utc>,
    },
}

fn drive_bitstreams(
    opts: &Opts,
    mut session: Session,
) -> (
    JoinHandle<Session>,
    UnboundedSender<OutgoingBit>,
    UnboundedReceiver<IncomingBit>,
) {
    let (outgoing_tx, mut outgoing_rx) = unbounded_channel();
    let (incoming_tx, incoming_rx) = unbounded_channel();

    let (self_party, other_party) = opts.parties();

    let handle = tokio::spawn(async move {
        // send/receive process
        let mut my_next_recv;
        let mut their_next_recv;
        let mut send_buffer = VecDeque::new();
        'process_loop: loop {
            my_next_recv = session.peek_next_rendezvous(self_party);
            their_next_recv = session.peek_next_rendezvous(other_party);
            let dt_now = Utc::now();
            if my_next_recv.read_by() <= dt_now {
                if let Err(_) = incoming_tx.send(IncomingBit::Miss {
                    receiving_party: self_party,
                    rendezvous_idx: my_next_recv.idx(),
                    rendezvous_time: my_next_recv.read_by(),
                    arrival_time: dt_now,
                }) {
                    tracing::debug!("incoming_rx closed");
                    break;
                }
                let _ = session.commit_rendezvous(self_party); // discard
                continue;
            }
            if their_next_recv.write_by() <= dt_now {
                if let Err(_) = incoming_tx.send(IncomingBit::Miss {
                    receiving_party: other_party,
                    rendezvous_idx: their_next_recv.idx(),
                    rendezvous_time: my_next_recv.write_by(),
                    arrival_time: dt_now,
                }) {
                    tracing::debug!("incoming_rx closed");
                    break;
                }
                let _ = session.commit_rendezvous(other_party); // discard
                continue;
            }
            let deadline_recv = if dt_now < my_next_recv.can_read_at() {
                Instant::now() + (my_next_recv.can_read_at() - dt_now).to_std().unwrap()
            } else {
                Instant::now()
            };
            let deadline_send = if dt_now < their_next_recv.can_write_at() {
                Instant::now()
                    + (their_next_recv.can_write_at() - dt_now)
                        .to_std()
                        .unwrap()
            } else {
                Instant::now()
            };
            let sleep_recv = sleep_until(deadline_recv);
            let sleep_send = sleep_until(deadline_send);
            tokio::select! {
                outgoing = outgoing_rx.recv() => {
                    match outgoing {
                        Some(OutgoingBit::Close) | /* channel was closed */ None => {
                            break 'process_loop
                        }
                        Some(OutgoingBit::Bit(b)) => {
                            send_buffer.push_back(b);
                        }
                    }
                }
                _ = sleep_recv => {
                    session.commit_rendezvous(self_party); // commit
                    tokio::spawn(raw_read_bit(session.resolver(), incoming_tx.clone(), my_next_recv, session.rtt()));
                }
                _ = sleep_send => {
                    session.commit_rendezvous(other_party); // commit
                    if let Some(bit) = send_buffer.pop_front() {
                        tokio::spawn(raw_write_bit(session.resolver(), bit, their_next_recv));
                    }
                }
            }
        }
        session
    });

    (handle, outgoing_tx, incoming_rx)
}

async fn raw_write_bit(resolver: SocketAddr, bit: bool, rendezvous: Rendezvous) {
    if bit {
        let _ =  raw_bit(resolver, rendezvous.domain(), /* arbitrary */ 0, Duration::from_millis(0)).await;
        // if ... {
        //     tracing::warn!("target domain {} already in cache", rendezvous.domain());
        // }
    }
    tracing::trace!("wrote bit #{}: {bit:?} -> {}", rendezvous.idx(), rendezvous.domain());
}
async fn raw_read_bit(
    resolver: SocketAddr,
    incoming_tx: UnboundedSender<IncomingBit>,
    rendezvous: Rendezvous,
    rtt: Duration,
) {
    let bit = raw_bit(resolver, rendezvous.domain(), (rendezvous.can_read_at() - rendezvous.write_by()).num_seconds() as u64, rtt).await;
    tracing::trace!("read bit #{}: {bit:?} <- {}", rendezvous.idx(), rendezvous.domain());
    let _ = incoming_tx.send(IncomingBit::Bit(rendezvous.idx(), bit));
}

async fn raw_bit(resolver: SocketAddr, domain: &Domain<Buffer>, delta: u64, rtt: Duration) -> bool {
    let conn = UdpClientStream::builder(resolver, TokioRuntimeProvider::default()).build();
    let (mut client, bg) = Client::connect(conn).await.unwrap();
    let _ = tokio::spawn(bg);
    let name = Name::from_str(domain.into_inner().as_str()).expect("rendezvous domain is valid");
    // tracing::trace!("sending DNS A request for {}", domain);
    let t0 = Instant::now();
    let response = match client
        .query(name, DNSClass::IN, RecordType::A)
        .await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Error resolving DNS query ({domain}): {e:?}");
            return /* chosen to make it easier to pick up preambles */ true
        }
    };
    let t1 = Instant::now();
    let query_time = t1 - t0;
    if delta != 0 {
        tracing::trace!("{domain} rc={} qt={query_time:?} ttl={:?}",
            response.response_code(),
            response.answers().first().map(|a| a.ttl())
        );
    }
    if let Some(ans) = response.answers().first() {
        // let sussy_neg = [30, 50];
        let sussy_pos = [60, 100];

        let dns_ttl = ans.ttl();

        // if sussy_neg.iter().any(|t| dns_ttl.is_multiple_of(*t) || (dns_ttl + 1).is_multiple_of(*t)) {
        //     return false
        // }
        if dns_ttl.is_multiple_of(30) || (dns_ttl + 1).is_multiple_of(30) {
            return false
        }
        // We forbid the 49/50 cases since, with our delta of 10sec, 60sec TTLs decay to the 50
        // range, causing false zeroes
        if (dns_ttl.is_multiple_of(50) || (dns_ttl + 1).is_multiple_of(50)) && (dns_ttl + 49) / 50 > 1 {
            return false
        }
        return sussy_pos.iter().any(|t| {
            ((dns_ttl + (*t - 1)) / *t * *t) - dns_ttl
                <=
            ((query_time.as_millis() + 999) / 1000 * 1000) as u32
        });
    }
    if let Some(soa) = response.soa()
        // && matches!(response.response_code(), ResponseCode::NXDomain)
    {
        tracing::trace!("{domain} SOA.TTL={} SOA.MINIMUM={} delta={}", soa.ttl(), soa.data().minimum(), delta);
        if soa.ttl().is_multiple_of(30) || soa.ttl().is_multiple_of(50) {
            return false
        }
        return (soa.data().minimum() - soa.ttl()) as u64 >= (delta - 2)
    }
    // CASE: ServFail - no information can be extracted, so just ignore it
    query_time <= rtt
}
