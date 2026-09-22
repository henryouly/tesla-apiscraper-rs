//! Decoupled InfluxDB writer (see issue #42).
//!
//! A dedicated task drains a bounded queue, so the vehicle event loop never
//! awaits the network: a slow database used to stall polling, command
//! handling, and WebSocket reads (including pongs) for up to the 5s client
//! timeout per write.
//!
//! Ingestion policy:
//! - Telemetry (positions, charge readings): `try_send`, dropped with a
//!   counter when the queue is full. Callers should keep their dedup state
//!   on drop so the next tick retries, mirroring the old write-error path.
//! - Session records (drive/charge/update summaries): queued with
//!   backpressure — the send only waits when the queue is completely full,
//!   which means the writer is stuck, not merely slow.
//! - [`DbWriter::flush`] barriers the queue; the task loop awaits it on
//!   shutdown so the final session summary is not lost on restart.
//! - [`DbWriter::shutdown`] is the bounded variant used at shutdown: it
//!   first marks the writer closing so stale telemetry is skipped, bounding
//!   the drain by the few session records instead of the whole backlog.
//!
//! Accepted tradeoff (vs. `docs/constitution/goal.md` success criterion 2,
//! "without backpressure or data loss"): under a sustained outage longer
//! than the queue absorbs, telemetry is dropped rather than spooled. The
//! previous code lost the same points (failed writes are gone either way)
//! while additionally stalling the event loop; a durable disk spool is
//! deferred until DB-outage durability proves worth its complexity.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use tracing::{debug, warn};

use crate::influxdb::{InfluxDb, Precision};

/// Default queue depth: ~4 minutes of 4Hz streaming telemetry.
pub(crate) const DEFAULT_CAPACITY: usize = 1024;

enum Write {
    Line {
        lp: String,
        session: bool,
        precision: Precision,
    },
    Barrier(tokio::sync::oneshot::Sender<()>),
}

/// Sender side of the decoupled writer. Construct per vehicle task via
/// [`DbWriter::new`]; cheap to clone — all handles feed the same queue, and
/// the writer task exits once all handles are dropped (after draining).
#[derive(Clone, Debug)]
pub(crate) struct DbWriter {
    tx: tokio::sync::mpsc::Sender<Write>,
    dropped_telemetry: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
}

impl DbWriter {
    pub fn new(db: Arc<InfluxDb>, capacity: usize) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel(capacity.max(1));
        let closing = Arc::new(AtomicBool::new(false));
        let closing_task = Arc::clone(&closing);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_task = Arc::clone(&dropped);
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    Write::Line {
                        lp,
                        session,
                        precision,
                    } => {
                        // After shutdown starts, stale telemetry is worthless
                        // (superseded by newer state) — skip it so the drain
                        // stays bounded by the few session records. Session
                        // lines still go through. Counting skips as dropped
                        // keeps the loss visible.
                        if !session && closing_task.load(Ordering::Relaxed) {
                            dropped_task.fetch_add(1, Ordering::Relaxed);
                            debug!("db_writer: telemetry skipped (shutting down)");
                            continue;
                        }
                        if let Err(e) = db.write_lp(&lp, precision).await {
                            warn!(error = %e, "db_writer: WRITE FAILED");
                        }
                    }
                    Write::Barrier(ack) => {
                        let _ = ack.send(());
                    }
                }
            }
            debug!("db_writer: shut down");
        });
        Self {
            tx,
            dropped_telemetry: dropped,
            closing,
        }
    }

    /// Queue telemetry. Returns `false` (and counts) when dropped on a full
    /// queue — never blocks.
    pub fn send_telemetry(&self, lp: String, precision: Precision) -> bool {
        match self.tx.try_send(Write::Line {
            lp,
            session: false,
            precision,
        }) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let n = self.dropped_telemetry.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n.is_multiple_of(100) {
                    warn!(dropped = n, "db_writer: telemetry DROPPED (queue full)");
                } else {
                    debug!(dropped = n, "db_writer: telemetry dropped (queue full)");
                }
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Queue a session record. Only waits when the queue is completely full;
    /// returns `false` solely when the writer task is gone.
    pub async fn send_session(&self, lp: String, precision: Precision) -> bool {
        self.tx
            .send(Write::Line {
                lp,
                session: true,
                precision,
            })
            .await
            .is_ok()
    }

    /// Block until all previously queued writes complete.
    pub async fn flush(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(Write::Barrier(tx)).await.is_err() {
            return; // writer gone: nothing left to flush
        }
        let _ = rx.await;
    }

    /// Bounded shutdown drain: mark closing so queued telemetry is skipped,
    /// then barrier until the remaining session records complete. Stale
    /// telemetry is superseded by newer state anyway; session summaries are
    /// what must survive a restart.
    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::Relaxed);
        self.flush().await;
    }

    pub fn dropped_telemetry(&self) -> u64 {
        self.dropped_telemetry.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer_for(url: &str, capacity: usize) -> DbWriter {
        DbWriter::new(
            Arc::new(InfluxDb::new(url, "", "", "test").unwrap()),
            capacity,
        )
    }

    #[tokio::test]
    async fn delivers_queued_writes_in_order() {
        let server = wiremock::MockServer::start().await;
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let bodies_clone = Arc::clone(&bodies);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/write"))
            .respond_with(move |req: &wiremock::Request| {
                bodies_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(String::from_utf8_lossy(&req.body).into_owned());
                wiremock::ResponseTemplate::new(204)
            })
            .mount(&server)
            .await;

        let writer = writer_for(&server.uri(), 16);
        assert!(writer.send_telemetry("positions value=1i 100".into(), Precision::Seconds));
        assert!(
            writer
                .send_session("drives value=2i 101".into(), Precision::Seconds)
                .await
        );
        writer.flush().await;

        let bodies = bodies.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("positions"));
        assert!(bodies[1].contains("drives"));
    }

    #[tokio::test]
    async fn drops_telemetry_on_full_queue() {
        // Capacity 1 with an unreachable DB: the tight loop below outruns
        // the writer task, so the queue fills and sends start dropping
        // instead of blocking.
        let writer = writer_for("http://localhost:1", 1);
        for i in 0..10_000 {
            if !writer.send_telemetry(format!("positions value={i}i 100"), Precision::Seconds) {
                break;
            }
        }
        assert!(
            writer.dropped_telemetry() > 0,
            "expected drops on a full queue"
        );
    }

    #[tokio::test]
    async fn cloned_handles_share_one_queue() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .expect(2)
            .mount(&server)
            .await;

        let writer = writer_for(&server.uri(), 16);
        let clone = writer.clone();
        assert!(writer.send_telemetry("positions value=1i 100".into(), Precision::Seconds));
        assert!(
            clone
                .send_session("drives value=2i 101".into(), Precision::Seconds)
                .await
        );
        writer.flush().await;
        assert_eq!(writer.dropped_telemetry(), 0);
    }

    #[tokio::test]
    async fn shutdown_skips_backlog_but_keeps_session() {
        // Slow DB (2s per write): the writer is still busy with the first
        // telemetry when shutdown starts, so 4 more telemetry lines plus a
        // session record pile up behind it.
        let server = wiremock::MockServer::start().await;
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let bodies_clone = Arc::clone(&bodies);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/write"))
            .respond_with(move |req: &wiremock::Request| {
                bodies_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(String::from_utf8_lossy(&req.body).into_owned());
                wiremock::ResponseTemplate::new(204).set_delay(std::time::Duration::from_secs(2))
            })
            .mount(&server)
            .await;

        let writer = writer_for(&server.uri(), 16);
        for i in 0..5 {
            assert!(writer.send_telemetry(format!("positions value={i}i 100"), Precision::Seconds));
        }
        assert!(
            writer
                .send_session("drives value=9i 109".into(), Precision::Seconds)
                .await
        );

        // Full drain would take ~12s (5 telemetry + session at 2s each);
        // shutdown must skip the stale telemetry and finish in ~4s.
        let start = std::time::Instant::now();
        writer.shutdown().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(9),
            "shutdown not bounded, took {elapsed:?}"
        );

        assert!(
            writer.dropped_telemetry() > 0,
            "expected stale telemetry to be skipped"
        );
        let bodies = bodies.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            bodies.iter().any(|b| b.contains("drives")),
            "session record must survive shutdown, got: {bodies:?}"
        );
    }
}
