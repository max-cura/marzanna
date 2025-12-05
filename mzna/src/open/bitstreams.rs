use std::collections::VecDeque;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use chrono::{DateTime, Utc};
use hickory_client::client::{Client, ClientHandle};
use hickory_client::proto::rr::{DNSClass, Name, RecordType};
use hickory_client::proto::runtime::TokioRuntimeProvider;
use hickory_client::proto::udp::UdpClientStream;
use hostaddr::{Buffer, Domain};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};
use marzanna::Rendezvous;
use marzanna::session::{Party, Session};
use crate::open::Opts;

#[derive(Debug)]
pub enum OutgoingFrame {
    Frame([u8; 64]),
    Close,
}
#[derive(Debug)]
pub enum IncomingFrame {
    Frame(usize, [u8; 64]),
    // Miss {
    //     receiving_party: Party,
    //     rendezvous_idx: usize,
    //     rendezvous_time: DateTime<Utc>,
    //     arrival_time: DateTime<Utc>,
    // },
}

pub fn drive(
    opts: &Opts,
    mut session: Session,
) -> (
    JoinHandle<Session>,
    UnboundedSender<OutgoingFrame>,
    UnboundedReceiver<IncomingFrame>,
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
                if let Err(_) = incoming_tx.send(IncomingFrame::Miss {
                    receiving_party: self_party,
                    rendezvous_idx: my_next_recv.idx(),
                    rendezvous_time: my_next_recv.read_by(),
                    arrival_time: dt_now,
                }) {
                    tracing::debug!("incoming_rx closed");
                    break;
                }
                let _ = session.commit_rendezvous(self_party); // discard
                break;
            }
            if their_next_recv.write_by() <= dt_now {
                if let Err(_) = incoming_tx.send(IncomingFrame::Miss {
                    receiving_party: other_party,
                    rendezvous_idx: their_next_recv.idx(),
                    rendezvous_time: my_next_recv.write_by(),
                    arrival_time: dt_now,
                }) {
                    tracing::debug!("incoming_rx closed");
                    break;
                }
                let _ = session.commit_rendezvous(other_party); // discard
                break;
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
                        Some(OutgoingFrame::Close) | /* channel was closed */ None => {
                            break 'process_loop
                        }
                        Some(OutgoingFrame::Frame(b)) => {
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
        let _ = raw_bit(resolver, rendezvous.domain(), /* arbitrary */ 0, Duration::from_millis(0)).await;
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
