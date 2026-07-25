//! Fan-out from the engine to every connected client.
//!
//! The engine reports through a single [`Emitter`]; there may be any number of
//! UIs attached. `Hub` implements that trait by publishing onto a broadcast
//! channel, and each client's event stream takes its own receiver.
//!
//! Deliberately dumb: no batching, no dedup here. Those are per-client concerns
//! (see `api::events`) because a client that connected five seconds ago and one
//! that connected just now need different things, and because a shared buffer
//! would let a slow client's cadence dictate a fast one's.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moin_core::category::Category;
use moin_core::engine::Emitter;
use moin_core::settings::Settings;
use moin_core::task::{Task, TaskProgress};
use tokio::sync::broadcast;

/// How many events a lagging client may fall behind before the channel starts
/// dropping them. Generous: at a few thousand active tasks a burst of raw
/// progress ticks is normal. A client that still overruns it gets a fresh
/// snapshot rather than a hole in its state (see `api::events`).
const CHANNEL_CAPACITY: usize = 8192;

/// One thing that happened, on its way to every client.
#[derive(Clone, Debug)]
pub enum Event {
    Added(Task),
    /// A raw, per-tick reading. Clients coalesce these themselves.
    Progress(TaskProgress),
    Updated(Task),
    Removed(String),
    Completed(Task),
    ToolProgress {
        received: u64,
        total: Option<u64>,
    },
    SettingsChanged(Settings),
    CategoriesChanged(Vec<Category>),
}

pub struct Hub {
    tx: broadcast::Sender<Event>,
    /// Live client count. Tracked explicitly rather than read off the broadcast's
    /// receiver count, because that also counts receivers held for other reasons
    /// and this number decides when the daemon exits.
    clients: AtomicUsize,
    /// Whether any client has *ever* attached. Until one has, an empty count means
    /// "still starting up", not "everyone left".
    seen_client: AtomicUsize,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Arc::new(Self {
            tx,
            clients: AtomicUsize::new(0),
            seen_client: AtomicUsize::new(0),
        })
    }

    pub fn publish(&self, event: Event) {
        // The only error is "no receivers", which is normal and fine.
        let _ = self.tx.send(event);
    }

    /// Take a receiver and register as a client. The returned guard decrements the
    /// count when the stream is dropped, however it ends.
    pub fn subscribe(self: &Arc<Self>) -> (broadcast::Receiver<Event>, ClientGuard) {
        let rx = self.tx.subscribe();
        self.clients.fetch_add(1, Ordering::SeqCst);
        self.seen_client.fetch_add(1, Ordering::SeqCst);
        (rx, ClientGuard { hub: self.clone() })
    }

    pub fn client_count(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    pub fn has_ever_had_a_client(&self) -> bool {
        self.seen_client.load(Ordering::SeqCst) > 0
    }
}

/// Drops a client from the count when its stream ends — normally, by navigation,
/// or because the process went away.
pub struct ClientGuard {
    hub: Arc<Hub>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.hub.clients.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Emitter for Hub {
    fn added(&self, task: &Task) {
        self.publish(Event::Added(task.clone()));
    }
    fn progress(&self, p: &TaskProgress) {
        self.publish(Event::Progress(p.clone()));
    }
    fn updated(&self, task: &Task) {
        self.publish(Event::Updated(task.clone()));
    }
    fn removed(&self, id: &str) {
        self.publish(Event::Removed(id.to_string()));
    }
    fn completed(&self, task: &Task) {
        // Notifications belong to whatever has a desktop session; the daemon has
        // none. The engine already gated this on the user's setting, so a client
        // just shows what arrives.
        self.publish(Event::Completed(task.clone()));
    }
}
