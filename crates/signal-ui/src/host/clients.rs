//! The clients: sending to one or all through their bounded queues, eviction
//! and removal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use serde_json::Value;

use crate::server::Outbound;

use super::Host;

pub(super) struct Client {
    pub(super) queue: SyncSender<Outbound>,
    pub(super) evicted: Arc<AtomicBool>,
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub(super) gate: Arc<AtomicBool>,
    pub(super) dropped: u64,
}

impl Host {
    pub(super) fn send_text(&mut self, client: u64, value: &Value) {
        self.send_serialised(client, value.to_string());
    }

    fn send_serialised(&mut self, client: u64, text: String) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        match c.queue.try_send(Outbound::Text(text)) {
            Ok(()) => {}
            // A client that cannot take a reply is not keeping up.
            Err(TrySendError::Full(_)) => self.evict(client),
            Err(TrySendError::Disconnected(_)) => self.remove(client),
        }
    }

    pub(super) fn send_binary(&mut self, client: u64, bytes: Vec<u8>) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        match c.queue.try_send(Outbound::Binary(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => c.dropped += 1,
            Err(TrySendError::Disconnected(_)) => self.remove(client),
        }
    }

    pub(super) fn broadcast(&mut self, value: &Value) {
        let clients: Vec<u64> = self.clients.keys().copied().collect();
        for client in clients {
            self.send_text(client, value);
        }
    }

    /// A message already serialised, to every client.
    pub(super) fn broadcast_text(&mut self, text: &str) {
        let clients: Vec<u64> = self.clients.keys().copied().collect();
        for client in clients {
            self.send_serialised(client, text.to_owned());
        }
    }

    /// Flags the client and drops its queue: its thread sees either and
    /// ends on its own; nothing here waits for it.
    fn evict(&mut self, client: u64) {
        if let Some(c) = self.clients.remove(&client) {
            c.evicted.store(true, Ordering::Relaxed);
            self.evicted += 1;
        }
        self.release(client);
        self.end_notes(client);
    }

    pub(super) fn remove(&mut self, client: u64) {
        self.clients.remove(&client);
        self.release(client);
        self.end_notes(client);
    }
}
