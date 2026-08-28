//! Is that server actually up? A TCP connect to the database's own port, in
//! the background, so the Connections list can say "up" or "down" before you
//! press Enter instead of leaving you staring at a connect that will time out.
//!
//! This is a reachability check, not an authentication one and not a query: a
//! green dot means something is listening on that port, which is exactly the
//! question you have when a connect hangs. Whether your password is right, or
//! whether `pg_hba.conf` will have you, is what pressing Enter finds out. Probes run off the UI thread and report back over a
//! channel; results carry the generation they were started in, so answers from
//! a superseded round are discarded rather than repainting a stale list.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// How many servers we probe at once. A list with a hundred entries should not
/// open a hundred sockets and a hundred threads at the same time.
const BATCH: usize = 8;

/// What we know about one server's port right now.
#[derive(Clone, Copy, PartialEq)]
pub enum Reach {
    /// A probe is in flight.
    Probing,
    /// The port accepted a connection, in this many milliseconds.
    Up(u64),
    /// Refused, unroutable, unresolvable or slower than `TIMEOUT`.
    Down,
}

/// One probe's answer, tagged with the round that asked for it.
pub struct Msg {
    pub key: String,
    pub generation: u64,
    pub reach: Reach,
}

/// A server to probe: the connection key we report under, and its address.
pub struct Target {
    pub key: String,
    pub host: String,
    pub port: u16,
}

/// Probe every target in the background, `BATCH` at a time, sending each answer
/// as it lands. Returns immediately; the caller drains `tx`'s receiver.
pub fn probe_all(targets: Vec<Target>, generation: u64, tx: Sender<Msg>, timeout_secs: u64) {
    // Long enough for a server on the far side of a VPN, short enough that a
    // dead one resolves while you are still looking at the list. Yours to change
    // in Settings, because only you know which of those two you are.
    let timeout = Duration::from_secs(timeout_secs.clamp(1, 60));
    if targets.is_empty() {
        return;
    }
    thread::spawn(move || {
        for chunk in targets.chunks(BATCH) {
            // Scoped threads let the batch borrow `chunk` and guarantee they are
            // all finished before the next batch starts, which is what bounds
            // the concurrency without a pool.
            thread::scope(|s| {
                for t in chunk {
                    let tx = tx.clone();
                    s.spawn(move || {
                        let reach = probe(&t.host, t.port, timeout);
                        // A closed receiver just means the TUI is gone; there is
                        // nothing to report to and nothing to clean up.
                        let _ = tx.send(Msg {
                            key: t.key.clone(),
                            generation,
                            reach,
                        });
                    });
                }
            });
        }
    });
}

/// One TCP connect with a deadline. DNS resolution has no timeout of its own,
/// so a name with broken DNS can hold its thread until the resolver gives up;
/// the answer is discarded by generation if the round has moved on.
fn probe(host: &str, port: u16, timeout: Duration) -> Reach {
    let started = Instant::now();
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return Reach::Down;
    };
    // Try each address (a name can resolve to both A and AAAA records); the
    // first one that answers is the one the client would have used.
    for addr in addrs.by_ref() {
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return Reach::Up(started.elapsed().as_millis() as u64);
        }
    }
    Reach::Down
}
