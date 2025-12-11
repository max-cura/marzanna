use aes_gcm_siv::Aes256GcmSiv;
use aes_gcm_siv::aead::Aead;
use bytes::{BufMut, Bytes, BytesMut};
use chrono::Utc;
use clap::Args;
use eyre::Context;
use hickory_client::client::{Client, ClientHandle};
use hickory_client::proto::rr::{DNSClass, Name, RecordType};
use hickory_client::proto::runtime::TokioRuntimeProvider;
use hickory_client::proto::udp::UdpClientStream;
use hostaddr::{Buffer, Domain};
use labrador_ldpc::LDPCCode;
use marzanna::Rendezvous;
use marzanna::codec::{Codec, Simple1x1};
use marzanna::session::{Party, Session};
use rand::RngCore;
use rand_chacha::ChaCha20Rng;
use rustyline_async::{Readline, ReadlineEvent, SharedWriter};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing::Level;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::{MakeWriter, layer};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer, Registry};

#[derive(Debug, Args)]
pub struct Opts {
    #[arg(short = 's')]
    session: PathBuf,
    #[arg(long)]
    unsafe_free_party: bool,

    #[arg(long)]
    local_socket: Option<PathBuf>,

    #[arg(long)]
    log_main: PathBuf,
    #[arg(long)]
    log_machine: PathBuf,
}
impl Opts {
    pub(crate) fn parties(&self) -> (Party, Party) {
        if self.unsafe_free_party {
            (Party::Bob, Party::Alice)
        } else {
            (Party::Alice, Party::Bob)
        }
    }
}

const PEWTER: &'static str = "pewter";

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    let session_file_contents =
        std::fs::read_to_string(&opts.session).wrap_err("failed to read session file")?;
    let mut session: Session =
        serde_json::from_str(&session_file_contents).wrap_err("failed to parse session file")?;

    let file_gen = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&opts.log_main)
        .wrap_err(eyre::eyre!(
            "human-readable log file {} could not be created",
            opts.log_machine.display()
        ))?;
    let file_targ = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&opts.log_machine)
        .wrap_err(eyre::eyre!(
            "machine-readable log file {} could not be created",
            opts.log_machine.display()
        ))?;

    let (nb_gen, _guard_gen) = tracing_appender::non_blocking(file_gen);
    let (nb_targ, _guard_targ) = tracing_appender::non_blocking(file_targ);

    Registry::default()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(nb_gen)
                // .flatten_event(true)
                .with_filter(EnvFilter::from_default_env()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(nb_targ)
                .json()
                .with_filter(Targets::new().with_target(PEWTER, Level::INFO)),
        )
        .try_init()
        .wrap_err(eyre::eyre!(
            "failed to initialize tracing-subscriber Registry"
        ))?;

    let (out_msg_tx, out_msg_rx) = unbounded_channel();
    let (in_msg_tx, in_msg_rx) = unbounded_channel();
    let hdl_stdio = drive_stdio(out_msg_tx, in_msg_rx);

    // let codec = session.codec_mut().clone();
    // let shared_key = session.shared_key().to_vec();

    let recv_upto = session.peek_next_rendezvous(opts.parties().0).idx();

    // let (hdl_session, bit_tx, bit_rx) = bitstreams::drive(&opts, session);
    let hdl_session = drive_protocol(&opts, session, recv_upto, out_msg_rx, in_msg_tx);

    let _session = hdl_session.await.wrap_err("bit driver panicked")?;
    // let codec = hdl_codec.await.wrap_err("protocol driver panicked")?;
    let () = hdl_stdio.await.wrap_err("stdio driver panicked")?;

    // *session.codec_mut() = codec;

    Ok(())
}

fn drive_stdio(
    msg_tx: UnboundedSender<OutgoingMsg>,
    mut msg_rx: UnboundedReceiver<IncomingMsg>,
) -> JoinHandle<()> {
    let (mut rl, mut stdout) =
        Readline::new("\x1b[33m>\x1b[0m ".into()).expect("failed to create readline");
    // struct SharedWriterWrapper(SharedWriter);
    // impl<'a> MakeWriter<'a> for SharedWriterWrapper {
    //     type Writer = SharedWriter;

    //     fn make_writer(&'a self) -> Self::Writer {
    //         self.0.clone()
    //     }
    // }
    // tracing_subscriber::fmt()
    //     .with_writer(SharedWriterWrapper(stdout.clone()))
    //     .with_env_filter(EnvFilter::from_default_env())
    //     .init();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = msg_rx.recv() => {
                    match msg {
                        Some(IncomingMsg::Msg(bytes)) => {
                            let _ = writeln!(stdout, "Received message: {}", hex::encode(&bytes));
                            let _ = writeln!(stdout, "(as ASCII): {}", String::from_utf8_lossy(bytes.as_ref()));
                        }
                        // Some(IncomingMsg::Miss { receiving_party,rendezvous_idx,rendezvous_time: _,arrival_time: _ }) => {
                        //     // tracing::warn!("missed {} {} window",
                        //     //     rendezvous_idx.to_ordinal_string(),
                        //     //     if receiving_party == self_party { "receive" } else { "send" }
                        //     // );
                        // }
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
    // Miss {
    //     receiving_party: Party,
    //     rendezvous_idx: usize,
    //     #[allow(unused)]
    //     rendezvous_time: DateTime<Utc>,
    //     #[allow(unused)]
    //     arrival_time: DateTime<Utc>,
    // },
}

fn drive_protocol(
    opts: &Opts,
    mut session: Session,
    // outgoing_tx: UnboundedSender<OutgoingBit>,
    // mut incoming_rx: UnboundedReceiver<IncomingBit>,
    // mut codec: DynCodec,
    // shared_key: Vec<u8>,
    recv_is_up_to_idx: usize,
    mut out_msg_rx: UnboundedReceiver<OutgoingMsg>,
    in_msg_tx: UnboundedSender<IncomingMsg>,
) -> JoinHandle<Session> {
    let (self_party, other_party) = opts.parties();

    let handle = tokio::spawn(async move {
        let mut bb_upto: usize = recv_is_up_to_idx;
        let mut bit_buffer: VecDeque<bool> = VecDeque::new();
        let mut receive_ahead: BTreeMap<usize, bool> = BTreeMap::new();

        const CELL_SIZE: usize = 64usize;
        let mut cell_seq: VecDeque<(usize, Vec<bool>)> = VecDeque::new();
        let mut out_cell_id = 0;
        let mut send_cell_id = 0;
        let mut recv_cell_id = 0;

        let mut my_next_recv;
        let mut their_next_recv;

        let mut send_buffer: VecDeque<bool> = VecDeque::new();

        let (incoming_tx, mut incoming_rx) = unbounded_channel::<(usize, bool)>();

        'outer: loop {
            my_next_recv = session.peek_next_rendezvous(self_party);
            their_next_recv = session.peek_next_rendezvous(other_party);
            // tracing::debug!("driver_protocol outer loop");
            let dt_now = Utc::now();
            if my_next_recv.read_by() <= dt_now {
                tracing::error!("missed receive window, closing");
                break;
            }
            if their_next_recv.write_by() <= dt_now {
                tracing::error!("missed send window, closing");
                break;
            }
            let deadline_recv = if dt_now < my_next_recv.can_read_at() {
                Instant::now() + (my_next_recv.can_read_at() - dt_now).to_std().unwrap()
            } else {
                Instant::now()
            };
            let deadline_send = if dt_now < their_next_recv.can_write_at() {
                Instant::now() + (their_next_recv.can_write_at() - dt_now).to_std().unwrap()
            } else {
                Instant::now()
            };
            let sleep_recv = sleep_until(deadline_recv);
            let sleep_send = sleep_until(deadline_send);

            tokio::select! {
                _ = sleep_recv => {
                    session.commit_materialized_rendezvous(self_party);
                    tokio::spawn(raw_read_bit(session.resolver(), incoming_tx.clone(), my_next_recv, session.rtt()));
                }
                _ = sleep_send => {
                    session.commit_materialized_rendezvous(other_party);
                    if send_buffer.is_empty() {
                        assert!(their_next_recv.idx().is_multiple_of(512));
                        if let Some(frame) = cell_seq.front() && frame.0 == send_cell_id {
                            send_buffer = cell_seq.pop_front().unwrap().1.into();
                        } else {
                            while let Some(cell) = cell_seq.front() && cell.0 < send_cell_id {
                                tracing::warn!("DISCARDING CELL {}", cell.0);
                                let _ = cell_seq.pop_front();
                            }
                            send_buffer = vec![false; CELL_SIZE * u8::BITS as usize].into();
                        }
                        send_cell_id += 1
                    }
                    let bit = send_buffer.pop_front().unwrap();
                    tokio::spawn(raw_write_bit(session.resolver(), bit, their_next_recv));
                }
                out_msg = out_msg_rx.recv() => {
                    if let Some(out_msg) = out_msg {
                        match out_msg {
                            OutgoingMsg::Msg(mut bytes) => {
                                while !bytes.is_empty() {
                                    let frame_payload = bytes.split_to(15.min(bytes.len()));
                                    let bits = build_cell(
                                        session.derive_per_cell_key(out_cell_id),
                                        session.cell_stream_cipher_mut(),
                                        out_cell_id,
                                        frame_payload);
                                    cell_seq.push_back((out_cell_id as usize, bits));
                                    out_cell_id += 1;
                                }
                            }
                            OutgoingMsg::Close => {
                                // let _ = outgoing_tx.send(OutgoingBit::Close);
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
                    let Some((idx, bit)) = in_bit else {
                        tracing::error!("incoming bit queue closed, stopping protocol driver.");
                        break
                    };
                    if idx == bb_upto {
                        bit_buffer.push_back(bit);
                        bb_upto += 1;
                    } else {
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
                    while bit_buffer.len() > (CELL_SIZE * u8::BITS as usize) {
                        let cell_bits = bit_buffer.drain(..CELL_SIZE * u8::BITS as usize).collect::<Vec<_>>();
                        let mut cell = BytesMut::new();
                        Simple1x1.decode(&cell_bits, &mut cell);
                        tracing::info!("raw cell: {}", hex::encode(&cell));

                        let mut xor = [0u8; 64];
                        session.cell_stream_cipher_mut().set_word_pos(recv_cell_id as u128 * 16);
                        session.cell_stream_cipher_mut().fill_bytes(&mut xor);
                        assert_eq!(session.cell_stream_cipher_mut().get_word_pos(), (recv_cell_id as u128 + 1) * 16);
                        assert_eq!(xor.len(), cell.len());
                        for (i, b) in cell.as_mut().iter_mut().enumerate() {
                            *b ^= xor[i];
                        }
                        tracing::info!("outer-unencrypted cell: {}", hex::encode(&cell));

                        assert_eq!(cell.len(), 64);
                        let mut llrs = vec![0i8; LDPCCode::TC512.n()];
                        LDPCCode::TC512.hard_to_llrs(cell.as_ref(), &mut llrs);
                        let mut working_u8 = vec![0u8; LDPCCode::TC512.decode_ms_working_u8_len()];
                        let mut working = vec![0i8; LDPCCode::TC512.decode_ms_working_len()];
                        let mut output = vec![0u8; LDPCCode::TC512.output_len()];
                        let (ok, iter) = LDPCCode::TC512.decode_ms(&llrs, &mut output, &mut working, &mut working_u8, 128);

                        tracing::info!("decoded cell (decoded={ok:?} after {iter}): {}", hex::encode(&output));

                        if ok {
                            let cipher = session.derive_per_cell_key(recv_cell_id);
                            recv_cell_id += 1;
                            match cipher.decrypt(b"12byte_nonce".into(), &output[..32]) {
                                Ok(pt) => {
                                    if let Err(_) = in_msg_tx.send(IncomingMsg::Msg(pt.into())) {
                                        tracing::debug!("incoming message queue closed, stopping protocol driver task");
                                        break 'outer
                                    }
                                }
                                Err(aes_gcm_siv::aead::Error) => {
                                    tracing::warn!("invalid payload tag, discarding message");
                                }
                            }

                            if let Err(_) = in_msg_tx.send(IncomingMsg::Msg(cell.freeze())) {
                                tracing::error!("incoming message queue closed, stopping protocol driver.");
                                break 'outer
                            }
                        } else {
                            tracing::error!("failed to decode cell!");
                        }
                    }
                }
            }
        }
        session
    });

    handle
}

fn build_cell(
    cipher: Aes256GcmSiv,
    cell_stream_cipher: &mut ChaCha20Rng,
    cell_id: u64,
    payload: Bytes,
) -> Vec<bool> {
    let payload_len = payload.len();
    assert!(payload_len <= 15);

    let mut frame = BytesMut::with_capacity(16);
    frame.put_u8(payload_len as u8);
    frame.extend(payload);
    while frame.len() < 16 {
        frame.put_u8(0);
    }

    let mut cipher_bytes = BytesMut::from(Bytes::from(
        cipher
            .encrypt(
                aes_gcm_siv::Nonce::from_slice(b"12byte_nonce"),
                frame.as_ref(),
            )
            .expect("failed to encrypt frame"),
    ));
    // tracing::debug!("cipher_bytes({}): {}", cipher_bytes.len(), hex::encode(&cipher_bytes));
    assert_eq!(cipher_bytes.len(), 32);
    cipher_bytes.put_bytes(0, 32);

    LDPCCode::TC512.encode(cipher_bytes.as_mut());

    tracing::info!("coded, AEAD-encrypted cell: {}", hex::encode(&cipher_bytes));

    // Cells are 512 bits = 64 bytes
    let mut xor = [0u8; 64];
    cell_stream_cipher.set_word_pos(cell_id as u128 * 16);
    cell_stream_cipher.fill_bytes(&mut xor);
    assert_eq!(
        cell_stream_cipher.get_word_pos(),
        (cell_id as u128 + 1) * 16
    );
    assert_eq!(xor.len(), cipher_bytes.len());
    for (i, b) in cipher_bytes.as_mut().iter_mut().enumerate() {
        *b ^= xor[i];
    }

    tracing::info!(
        "encrypted, coded, AEAD-encrypted cell: {}",
        hex::encode(&cipher_bytes)
    );

    let mut bits = vec![false; Simple1x1.required_bits(cipher_bytes.len())];

    Simple1x1.encode(&cipher_bytes.into(), &mut bits);
    bits
}

async fn raw_write_bit(resolver: SocketAddr, bit: bool, rendezvous: Rendezvous) {
    if bit {
        let _ = raw_bit(
            resolver,
            rendezvous.domain(),
            /* arbitrary */ 0,
            Duration::from_millis(0),
        )
        .await;
        // if ... {
        //     tracing::warn!("target domain {} already in cache", rendezvous.domain());
        // }
    }
    tracing::trace!(
        "wrote bit #{}: {bit:?} -> {}",
        rendezvous.idx(),
        rendezvous.domain()
    );
}
async fn raw_read_bit(
    resolver: SocketAddr,
    incoming_tx: UnboundedSender<(usize, bool)>,
    rendezvous: Rendezvous,
    rtt: Duration,
) {
    let bit = raw_bit(
        resolver,
        rendezvous.domain(),
        (rendezvous.can_read_at() - rendezvous.write_by()).num_seconds() as u64,
        rtt,
    )
    .await;
    tracing::trace!(
        "read bit #{}: {bit:?} <- {}",
        rendezvous.idx(),
        rendezvous.domain()
    );
    let _ = incoming_tx.send((rendezvous.idx(), bit));
}

async fn raw_bit(resolver: SocketAddr, domain: &Domain<Buffer>, delta: u64, rtt: Duration) -> bool {
    let conn = UdpClientStream::builder(resolver, TokioRuntimeProvider::default()).build();
    let (mut client, bg) = Client::connect(conn).await.unwrap();
    let _ = tokio::spawn(bg);
    let name = Name::from_str(domain.into_inner().as_str()).expect("rendezvous domain is valid");
    // tracing::trace!("sending DNS A request for {}", domain);
    let t0 = Instant::now();
    let response = match client.query(name, DNSClass::IN, RecordType::A).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Error resolving DNS query ({domain}): {e:?}");
            return /* chosen to make it easier to pick up preambles */ true;
        }
    };
    let t1 = Instant::now();
    let query_time = t1 - t0;
    if delta != 0 {
        tracing::trace!(
            "{domain} rc={} qt={query_time:?} ttl={:?}",
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
            return false;
        }
        // We forbid the 49/50 cases since, with our delta of 10sec, 60sec TTLs decay to the 50
        // range, causing false zeroes
        if (dns_ttl.is_multiple_of(50) || (dns_ttl + 1).is_multiple_of(50))
            && (dns_ttl + 49) / 50 > 1
        {
            return false;
        }
        return sussy_pos.iter().any(|t| {
            ((dns_ttl + (*t - 1)) / *t * *t) - dns_ttl
                <= ((query_time.as_millis() + 999) / 1000 * 1000) as u32
        });
    }
    if let Some(soa) = response.soa()
    // && matches!(response.response_code(), ResponseCode::NXDomain)
    {
        tracing::trace!(
            "{domain} SOA.TTL={} SOA.MINIMUM={} delta={}",
            soa.ttl(),
            soa.data().minimum(),
            delta
        );
        if soa.ttl().is_multiple_of(30) || soa.ttl().is_multiple_of(50) {
            return false;
        }
        return (soa.data().minimum() - soa.ttl()) as u64 >= (delta - 2);
    }
    // CASE: ServFail - no information can be extracted, so just ignore it
    query_time <= rtt
}
