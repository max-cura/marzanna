use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::{
    sync::{
        mpsc::{self, OwnedPermit, error::TrySendError},
        oneshot,
    },
    task::JoinHandle,
    time::{Instant, sleep_until},
};

use crate::{Rendezvous, Session};

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("missed rendezvous #{}, not processed until {}", rendezvous.idx, at)]
    Missed {
        rendezvous: Rendezvous,
        at: DateTime<Utc>,
    },
    #[error("event queue filled")]
    QueueFull,
    #[error("event queue closed")]
    QueueClosed,
}

pub fn drive<T: Send + 'static>(
    mut session: Session,
    conv_fn: impl Fn(&Rendezvous) -> (DateTime<Utc>, DateTime<Utc>) + Send + 'static,
    mut event_fn: impl FnMut(Rendezvous, OwnedPermit<Result<T, DriverError>>) + Send + 'static,
    event_send: mpsc::Sender<Result<T, DriverError>>,
    mut terminate_recv: oneshot::Receiver<()>,
) -> JoinHandle<(Session, Rendezvous, Result<(), DriverError>)> {
    tokio::spawn(async move {
        let mut rendezvous;
        loop {
            rendezvous = session.next_rendezvous();
            let event_permit = match event_send.clone().try_reserve_owned() {
                Ok(ev_p) => ev_p,
                Err(e) => {
                    break (
                        session,
                        rendezvous,
                        Err(match e {
                            TrySendError::Full(_) => DriverError::QueueFull,
                            TrySendError::Closed(_) => DriverError::QueueClosed,
                        }),
                    );
                }
            };
            let dt_now = Utc::now();
            let (dt_begin, dt_end) = conv_fn(&rendezvous);
            // tracing::debug!(
            //     "rendezvous.idk={} now={dt_now}, begin={dt_begin}, end={dt_end}",
            //     rendezvous.idx()
            // );
            if dt_now >= dt_end {
                event_permit.send(Err(DriverError::Missed {
                    rendezvous,
                    at: dt_now,
                }));
                continue;
            }
            let deadline = if dt_now < dt_begin {
                // PANICS: never; to_std() only errors if the TimeDelta is negative
                Instant::now() + (dt_begin - dt_now).to_std().unwrap()
            } else {
                Instant::now()
            };
            let approx_sleep = sleep_until(deadline);
            tokio::select! {
                _ = approx_sleep => {
                    event_fn(rendezvous, event_permit);
                }
                _ = &mut terminate_recv => {
                    break (session, rendezvous, Ok(()))
                }
            }
        }
    })
}
