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

pub async fn invoke(opts: Opts) -> eyre::Result<()> {
    let session_file_contents =
        std::fs::read_to_string(&opts.session).wrap_err("failed to read session file")?;
    let mut session: Session =
        serde_json::from_str(&session_file_contents).wrap_err("failed to parse session file")?;

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
                            tracing::warn!("missed {} {} window",
                                rendezvous_idx.to_ordinal_string(),
                                if receiving_party == self_party { "receive" } else { "send" }
                            );
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
    tracing::trace!(recv_is_up_to_idx);
    type Key = aes_gcm_siv::Key<Aes256GcmSiv>;
    const TAG_BYTES: usize = 16;
    assert_eq!(
        TAG_BYTES,
        <<Aes256GcmSiv as AeadCore>::TagSize as ToInt<usize>>::to_int()
    );
    static PREAMBLE_BITS: [bool; 24] = [
        // false, false, false, false,
        // false, false, false, false,
        true, true, true, true,
        true, true, true, true,
        true, true, true, true,
        true, true, true, true,
        true, true, true, true,
        true, true, false, true,
    ];
    fn maintain_preamble(buf: &mut VecDeque<bool>) -> bool {
        while buf.len() >= 24 {
            if let Some(first_diff) = buf.iter().zip(PREAMBLE_BITS.iter()).position(|(found, expect)| found != expect) {
                if first_diff == 22 {
                    buf.pop_front();
                } else if first_diff == 23 {
                    buf.truncate_front(buf.len() - 24);
                } else {
                    // 1111 1111 10 fd=9, truncate=10
                    buf.truncate_front(buf.len() - first_diff - 1);
                }
            } else {
                buf.truncate_front(buf.len() - 24);
                return true
            }
            // if let Some(first_true) = buf.iter().position(|&x| x == true) {
            //     // tracing::trace!("ft=={first_true}");
            //     if first_true == 4 {
            //         if let Some(first_diff) = buf.iter().zip(PREAMBLE_BITS.iter()).position(|(found, expected)| found != expected) {
            //             // tracing::trace!("fd=={first_diff}");
            //             if first_diff == 22 {
            //                 // Case: 0000 0000 1111 1111 1111 111 fd=22, remove 23
            //                 buf.truncate_front(buf.len() - 23);
            //             } else if first_diff == 23 {
            //                 // Case: 0000 0000 1111 1111 1111 1100 fd=23 remove 22
            //                 buf.truncate_front(buf.len() - 22);
            //             } else {
            //                 // Case: 0000 0000 110 fd=10, remove 10
            //                 buf.truncate_front(buf.len() - first_diff);
            //             }
            //         } else {
            //             // preamble matches
            //             buf.truncate_front(buf.len() - 24);
            //             return true
            //         }
            //     } else if first_true > 4 {
            //         // Example: 0000 0000 001 ft=10, remove 2
            //         let remove_extraneous_false_count = first_true - 4;
            //         buf.truncate_front(buf.len() - remove_extraneous_false_count);
            //     } else /* first_true < 4 */ {
            //         // Example: 0001 ft=3, remove 4
            //         buf.truncate_front(buf.len() - (first_true + 1));
            //     }
            // } else {
            //     // Case: entire buffer is `false`s, so truncate it to the last 8 `false`s
            //     buf.truncate_front(4);
            // }
        }
        false
    }
    // 96-bit nonces
    static LENGTH_NONCE: &[u8] = b"lengthlength";
    static PAYLOAD_NONCE: &[u8] = b"payloadpaylo";
    fn build_frame(shared_key: &[u8], codec: &mut DynCodec, out_msg: Bytes) -> Vec<bool> {
        let salt: [u8; SALT_BYTES] = rand_chacha::ChaCha20Rng::from_os_rng().random();
        let key = derive_key(Bytes::from_owner(salt), shared_key);
        let cipher = Aes256GcmSiv::new(&key);

        let mut frame = PREAMBLE_BITS.to_vec();
        let after = frame.len();

        // salt
        tracing::debug!("salt={}", hex::encode(&salt));
        frame.extend(std::iter::repeat_n(false, codec.required_bits(SALT_BYTES)));
        codec.encode(&Bytes::from_owner(salt), &mut frame[after..]);
        let after = frame.len();
        // tracing::debug!("frame={}", frame.iter().map(|x| if *x { "1" } else { "0" }).collect::<String>());

        // length + tag
        assert!(
            out_msg.len() < u32::MAX as usize,
            "outgoing message too large"
        );
        let len_bytes = Bytes::from(
            cipher
                .encrypt(
                    aes_gcm_siv::Nonce::from_slice(LENGTH_NONCE),
                    u32::try_from(out_msg.len())
                        .expect("length <= u32::MAX")
                        .to_le_bytes()
                        .as_slice(),
                )
                .expect("failed to encrypt"),
        );
        tracing::debug!("length+tag={}", hex::encode(&len_bytes));
        frame.extend(std::iter::repeat_n(false, codec.required_bits(len_bytes.len())));
        codec.encode(&len_bytes, &mut frame[after..]);
        let after = frame.len();

        // payload + tag
        let payload_bytes = Bytes::from(
            cipher
                .encrypt(PAYLOAD_NONCE.into(), out_msg.as_ref())
                .expect("failed to encrypt"),
        );
        tracing::debug!("payload+tag={}", hex::encode(&payload_bytes));
        frame.extend(std::iter::repeat_n(
            false,
            codec.required_bits(payload_bytes.len()),
        ));
        codec.encode(&payload_bytes, &mut frame[after..]);

        frame
    }
    const SALT_BYTES: usize = 16;
    fn derive_key(salt: Bytes, shared_key: &[u8]) -> Key {
        let mut okm = [0u8; 42];
        let derived = Hkdf::<Sha256>::new(Some(&salt[..]), shared_key);
        derived
            .expand(b"derived", &mut okm)
            .expect("42 is a valid length for Sha256 to output");
        // AES256-GCM-SIV wants a 32-byte key
        let mut key_bits = [0u8; 32];
        key_bits[..32].copy_from_slice(&okm[..32]);
        Key::from(key_bits)
    }
    fn maintain_bytes(codec: &mut DynCodec, buf: &mut VecDeque<bool>, len: usize) -> Option<Bytes> {
        let len_bits = codec.required_bits(len);
        if len_bits <= buf.len() {
            let take: Vec<bool> = buf.drain(..len_bits).collect();
            let mut o_buf = BytesMut::new();
            codec.decode(&take, &mut o_buf);
            // tracing::debug!("len={len} len_bits={} take={} o_buf={}", len_bits, take.iter().map(|x| if *x { "1" } else { "0" }).collect::<String>(), hex::encode(&o_buf));
            Some(o_buf.into())
        } else {
            None
        }
    }

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
                                let bits = build_frame(&shared_key, &mut codec, bytes);
                                for bit in bits {
                                    if let Err(_) = outgoing_tx.send(OutgoingBit::Bit(bit)) {
                                        tracing::debug!("outgoing bit queue closed, stopping protocol driver task");
                                        break
                                    }
                                }
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
                        if idx == bb_upto {
                            bit_buffer.push_back(bit);
                            bb_upto += 1;
                        } else {
                            tracing::trace!("mismatch: bb_upto={bb_upto} idx={idx} bit={bit:?}");
                            receive_ahead.insert(idx, bit);
                        }
                        while let Some(entry) = receive_ahead.first_entry() {
                            tracing::trace!("upto={} recv_ahead.fst=({},{:?})", bb_upto, *entry.key(), *entry.get());
                            if *entry.key() == bb_upto {
                                let (_idx, bit) = entry.remove_entry();
                                bit_buffer.push_back(bit);
                                bb_upto += 1;
                            } else {
                                break
                            }
                        }
                        tracing::trace!("bit_buf={}",
                            bit_buffer.iter().map(|x| if *x { "1" } else { "0" }).collect::<String>(),
                        );
                        loop {
                            // repeat the loop until the length of bit_buffer no longer changes
                            let bblen_mark = bit_buffer.len();
                            state = match state {
                                FS::Initial => {
                                    if maintain_preamble(&mut bit_buffer) {
                                        tracing::debug!("state change <- SALT");
                                        FS::Salt
                                    } else {
                                        FS::Initial
                                    }
                                },
                                FS::Salt => {
                                    if let Some(salt) = maintain_bytes(&mut codec, &mut bit_buffer, SALT_BYTES) {
                                        tracing::debug!("state change <- LEN, salt={}", hex::encode(&salt));
                                        let key = derive_key(salt, &shared_key);
                                        let cipher = Aes256GcmSiv::new(&key);
                                        FS::Salted {
                                            cipher,
                                            proto: PS::Len,
                                        }
                                    } else {
                                        FS::Salt
                                    }
                                },
                                FS::Salted { cipher, proto } => match proto {
                                    PS::Len => {
                                        if let Some(len_ct) = maintain_bytes(&mut codec, &mut bit_buffer, 4 + TAG_BYTES) {
                                            tracing::debug!("len_ct={}", hex::encode(&len_ct));
                                            match cipher.decrypt(LENGTH_NONCE.into(), len_ct.as_ref()) {
                                                Ok(pt) => {
                                                    let len_usize = u32::from_le_bytes(pt.try_into().expect("length ciphertext should decrypt to 4 bytes")) as usize;
                                                    tracing::debug!("state change <- PAYLOAD({len_usize})");
                                                    FS::Salted {
                                                        cipher,
                                                        proto: PS::Payload(len_usize),
                                                    }
                                                }
                                                Err(aes_gcm_siv::aead::Error) => {
                                                    tracing::warn!("invalid length tag, discarding message");
                                                    FS::Initial
                                                }
                                            }
                                        } else {
                                            FS::Salted { cipher, proto: PS::Len }
                                        }
                                    }
                                    PS::Payload(len) => {
                                        if let Some(payload_ct) = maintain_bytes(&mut codec, &mut bit_buffer, len + TAG_BYTES) {
                                            tracing::debug!("payload_ct={}", hex::encode(&payload_ct));
                                            match cipher.decrypt(PAYLOAD_NONCE.into(), payload_ct.as_ref()) {
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
                                            tracing::debug!("state change <- INITIAL");
                                            FS::Initial
                                        } else {
                                            FS::Salted { cipher, proto: PS::Payload(len) }
                                        }
                                    }
                                }
                            };
                            if bit_buffer.len() == bblen_mark {
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
        tracing::trace!("{domain} qt={query_time:?} ttl={:?}",
            response.answers().first().map(|a| a.ttl())
        );
    }
    if let Some(ans) = response.answers().first() {
        let dns_ttl = ans.ttl();
        if dns_ttl.is_multiple_of(30) || dns_ttl.is_multiple_of(50) {
            return false
        }
        let nearest_mul30 = (dns_ttl + 29) / 30 * 30;
        if (nearest_mul30 - dns_ttl) as u64 > query_time.as_secs() {
            return true
        }
        let nearest_mul50 = (dns_ttl + 49) / 50 * 50;
        if (nearest_mul50 - dns_ttl) as u64 > query_time.as_secs() {
            return true
        }
    }
    if let Some(soa) = response.soa() {
        return (soa.data().minimum() - soa.ttl()) as u64 >= delta
    }
    // if response.response_code() == ResponseCode::ServFail {
    //     return true
    // }
    // SOA_MIN-TTL>=delta only works for NXDOMAIN results since those include SOA
    // let soa = response.soa().unwrap().to_owned();
    // (soa.data().minimum() - soa.ttl()) as u64 >= delta
    query_time <= rtt
}
