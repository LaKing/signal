//! The clients: sending to one or all through their bounded queues, eviction
//! and removal.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use serde_json::{Value, json};

use crate::server::{Outbound, Scope};

use super::Host;

pub(super) struct Client {
    pub(super) queue: SyncSender<Outbound>,
    pub(super) evicted: Arc<AtomicBool>,
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub(super) gate: Arc<AtomicBool>,
    pub(super) dropped: u64,
    /// What its token lets it do (ADR 0053).
    pub(super) scope: Scope,
}

/// The connected clients by id, and how many were evicted. What a send
/// that fails means for the host (an eviction, a removal, and the client's
/// notes ended) is the host's ([`Host::send_text`]).
#[derive(Default)]
pub(super) struct Clients {
    by_id: BTreeMap<u64, Client>,
    evicted: u64,
    /// The panel-token clients gone (evicted or closed), whose messages
    /// still in the inbound queue or waiting for the writer lock must never
    /// run (ADR 0053: an unknown client is not given the session's
    /// authority). Ids are never reused.
    gone_panels: std::collections::BTreeSet<u64>,
}

/// What a send to a client did.
pub(super) enum Sent {
    /// Queued, or not: no such client, or a data frame dropped (counted).
    Done,
    /// Its queue is full.
    Full,
    /// Its connection has gone.
    Gone,
}

impl Clients {
    /// Client `client` connected.
    pub(super) fn connect(&mut self, client: u64, connected: Client) {
        self.by_id.insert(client, connected);
    }

    pub(super) fn contains(&self, client: u64) -> bool {
        self.by_id.contains_key(&client)
    }

    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub(super) fn get(&self, client: u64) -> Option<&Client> {
        self.by_id.get(&client)
    }

    /// What client `client`'s token lets it do.
    pub(super) fn scope(&self, client: u64) -> Option<&Scope> {
        self.by_id.get(&client).map(|c| &c.scope)
    }

    /// The clients of a project's panel token, with their scope.
    pub(super) fn scoped(&self) -> impl Iterator<Item = (u64, &Scope)> {
        self.by_id
            .iter()
            .filter(|(_, c)| matches!(c.scope, Scope::Project(_)))
            .map(|(id, c)| (*id, &c.scope))
    }

    /// The clients' ids, in order.
    pub(super) fn ids(&self) -> Vec<u64> {
        self.by_id.keys().copied().collect()
    }

    /// A control message to `client`: one its queue cannot take is
    /// [`Sent::Full`].
    pub(super) fn send_text(&self, client: u64, text: String) -> Sent {
        let Some(c) = self.by_id.get(&client) else {
            return Sent::Done;
        };
        match c.queue.try_send(Outbound::Text(text)) {
            Ok(()) => Sent::Done,
            Err(TrySendError::Full(_)) => Sent::Full,
            Err(TrySendError::Disconnected(_)) => Sent::Gone,
        }
    }

    /// A data frame to `client`: one its queue cannot take is dropped and
    /// counted.
    pub(super) fn send_binary(&mut self, client: u64, bytes: Vec<u8>) -> Sent {
        let Some(c) = self.by_id.get_mut(&client) else {
            return Sent::Done;
        };
        match c.queue.try_send(Outbound::Binary(bytes)) {
            Ok(()) => Sent::Done,
            Err(TrySendError::Full(_)) => {
                c.dropped += 1;
                Sent::Done
            }
            Err(TrySendError::Disconnected(_)) => Sent::Gone,
        }
    }

    /// Flags the client and drops its queue, counted.
    pub(super) fn evict(&mut self, client: u64) {
        if let Some(c) = self.by_id.remove(&client) {
            c.evicted.store(true, Ordering::Relaxed);
            self.evicted += 1;
            self.forget_panel(client, &c.scope);
        }
    }

    pub(super) fn remove(&mut self, client: u64) {
        if let Some(c) = self.by_id.remove(&client) {
            self.forget_panel(client, &c.scope);
        }
    }

    fn forget_panel(&mut self, client: u64, scope: &Scope) {
        if matches!(scope, Scope::Project(_)) {
            self.gone_panels.insert(client);
        }
    }

    /// Whether `client` was a panel-token client that is gone.
    pub(super) fn panel_gone(&self, client: u64) -> bool {
        self.gone_panels.contains(&client)
    }

    /// `stats`' `clients` (each one's dropped data frames) and `evicted`.
    pub(super) fn stats(&self) -> (Vec<Value>, u64) {
        let clients = self
            .by_id
            .iter()
            .map(|(id, c)| json!({"client": id, "dropped": c.dropped}))
            .collect();
        (clients, self.evicted)
    }
}

impl Host {
    pub(super) fn send_text(&mut self, client: u64, value: &Value) {
        self.send_serialised(client, value.to_string());
    }

    fn send_serialised(&mut self, client: u64, text: String) {
        match self.clients.send_text(client, text) {
            Sent::Done => {}
            // A client that cannot take a reply is not keeping up.
            Sent::Full => self.evict(client),
            Sent::Gone => self.remove(client),
        }
    }

    pub(super) fn send_binary(&mut self, client: u64, bytes: Vec<u8>) {
        match self.clients.send_binary(client, bytes) {
            Sent::Done | Sent::Full => {}
            Sent::Gone => self.remove(client),
        }
    }

    pub(super) fn broadcast(&mut self, value: &Value) {
        // A panel of a project no longer open hears nothing of the new one.
        self.sync_scope();
        for client in self.clients.ids() {
            self.send_text(client, value);
        }
    }

    /// A message already serialised, to every client.
    pub(super) fn broadcast_text(&mut self, text: &str) {
        self.sync_scope();
        for client in self.clients.ids() {
            self.send_serialised(client, text.to_owned());
        }
    }

    /// Flags the client and drops its queue: its thread sees either and
    /// ends on its own; nothing here waits for it.
    pub(super) fn evict(&mut self, client: u64) {
        self.clients.evict(client);
        self.release(client);
        self.end_notes(client);
    }

    pub(super) fn remove(&mut self, client: u64) {
        self.clients.remove(client);
        self.release(client);
        self.end_notes(client);
    }
}
