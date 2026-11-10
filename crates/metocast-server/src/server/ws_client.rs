//! One WebSocket client's outgoing queue.
//!
//! Every message for a client goes through a [`ClientTx`] into an unbounded queue that the
//! client's writer task empties into the socket. A full `presenter.state` can be hundreds of
//! kilobytes (an SVG deck with photos), and a screen on a slow link takes longer to receive one
//! than the operator takes to step to the next order item. So the presenter messages are tagged,
//! and the writer drops a queued deck, or a slide change, that a later queued `presenter.state`
//! already replaces ([`coalesce`]): a slow screen jumps straight to the current deck instead of
//! showing every deck it missed one after another. Everything else keeps its place and order.

use axum::extract::ws::Message;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use tokio::sync::mpsc;

/// What a queued message is, so the writer knows which ones a newer deck replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingKind {
    /// Kept and sent in order.
    Other,
    /// A `presenter.state`: the whole deck, including its current slide.
    PresenterState,
    /// A `presenter.slide_changed`: moves within the deck the client last got.
    PresenterSlide,
}

/// A message waiting in a client's queue.
#[derive(Debug)]
pub struct Outgoing {
    pub message: Message,
    pub kind: OutgoingKind,
    pub queued_at: Instant,
}

/// When the last presenter message the writer sent this client was queued and written: what a
/// `presenter.ack` is measured against, on the server's own clock.
#[derive(Debug, Clone, Copy)]
pub struct Delivered {
    pub queued_at: Instant,
    pub written_at: Instant,
}

/// The sending side of a client's queue. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ClientTx {
    tx: mpsc::UnboundedSender<Outgoing>,
    presenter_frames: bool,
    delivered: Arc<Mutex<Option<Delivered>>>,
}

impl ClientTx {
    /// A queue for a client. `presenter_frames` is true for a screen that shows SVG slides as
    /// JPEGs from `presenter.frame` (`/ws?presenter=frames`), so its `presenter.state` messages
    /// leave the SVGs out.
    pub fn channel(presenter_frames: bool) -> (Self, mpsc::UnboundedReceiver<Outgoing>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                tx,
                presenter_frames,
                delivered: Arc::default(),
            },
            rx,
        )
    }

    /// Queues a message that keeps its place.
    pub fn send(&self, message: Message) -> Result<(), mpsc::error::SendError<Outgoing>> {
        self.send_kind(message, OutgoingKind::Other)
    }

    /// Queues a message tagged with what it is.
    pub fn send_kind(
        &self,
        message: Message,
        kind: OutgoingKind,
    ) -> Result<(), mpsc::error::SendError<Outgoing>> {
        self.tx.send(Outgoing {
            message,
            kind,
            queued_at: Instant::now(),
        })
    }

    /// True for a client that draws SVG slides from `presenter.frame` and gets them left out.
    pub fn presenter_frames(&self) -> bool {
        self.presenter_frames
    }

    /// The last presenter message the writer sent this client.
    pub fn delivered(&self) -> Option<Delivered> {
        *self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a presenter message the writer just sent.
    pub fn set_delivered(&self, delivered: Delivered) {
        *self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(delivered);
    }
}

/// Drops every presenter message that the last queued `presenter.state` replaces: the earlier
/// decks, and the slide changes that moved within them. The last state carries its own current
/// slide, and slide changes queued after it stay, in order. Returns how many it dropped.
pub fn coalesce(pending: &mut VecDeque<Outgoing>) -> usize {
    let Some(last_state) = pending
        .iter()
        .rposition(|item| item.kind == OutgoingKind::PresenterState)
    else {
        return 0;
    };
    let before = pending.len();
    let mut index = 0;
    pending.retain(|item| {
        let keep = index >= last_state || item.kind == OutgoingKind::Other;
        index += 1;
        keep
    });
    before - pending.len()
}

/// Takes what is queued for the socket next: waits for one message when nothing is pending,
/// then moves in everything else already queued and coalesces it, with how many presenter
/// messages that dropped. None once the queue closed and nothing is left.
pub async fn next(
    rx: &mut mpsc::UnboundedReceiver<Outgoing>,
    pending: &mut VecDeque<Outgoing>,
) -> Option<(Outgoing, usize)> {
    if pending.is_empty() {
        pending.push_back(rx.recv().await?);
    }
    while let Ok(item) = rx.try_recv() {
        pending.push_back(item);
    }
    let dropped = coalesce(pending);
    pending.pop_front().map(|item| (item, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, kind: OutgoingKind) -> Outgoing {
        Outgoing {
            message: Message::Text(text.to_owned().into()),
            kind,
            queued_at: Instant::now(),
        }
    }

    fn texts(pending: &VecDeque<Outgoing>) -> Vec<String> {
        pending
            .iter()
            .map(|item| match &item.message {
                Message::Text(text) => text.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn newest_state_replaces_older_decks_and_their_slide_changes() {
        use OutgoingKind::*;
        let mut pending: VecDeque<Outgoing> = [
            item("ping", Other),
            item("deck A", PresenterState),
            item("A slide 2", PresenterSlide),
            item("order", Other),
            item("deck B", PresenterState),
            item("B slide 2", PresenterSlide),
            item("B slide 3", PresenterSlide),
            item("clients", Other),
        ]
        .into();
        assert_eq!(coalesce(&mut pending), 2, "deck A and its slide change");
        assert_eq!(
            texts(&pending),
            [
                "ping",
                "order",
                "deck B",
                "B slide 2",
                "B slide 3",
                "clients"
            ]
        );
    }

    #[test]
    fn slide_changes_without_a_newer_state_are_kept() {
        use OutgoingKind::*;
        let mut pending: VecDeque<Outgoing> = [
            item("slide 2", PresenterSlide),
            item("slide 3", PresenterSlide),
            item("ping", Other),
        ]
        .into();
        assert_eq!(coalesce(&mut pending), 0);
        assert_eq!(texts(&pending), ["slide 2", "slide 3", "ping"]);
    }

    #[tokio::test]
    async fn a_slow_writer_skips_straight_to_the_newest_deck() {
        let (tx, mut rx) = ClientTx::channel(false);
        let mut pending = VecDeque::new();
        tx.send_kind(Message::Text("deck 1".into()), OutgoingKind::PresenterState)
            .expect("queue open");
        // The writer takes deck 1 and is busy sending it while three more steps happen.
        let (first, dropped) = next(&mut rx, &mut pending).await.expect("deck 1");
        assert_eq!(
            (first.message, dropped),
            (Message::Text("deck 1".into()), 0)
        );
        for deck in ["deck 2", "deck 3", "deck 4"] {
            tx.send_kind(Message::Text(deck.into()), OutgoingKind::PresenterState)
                .expect("queue open");
        }
        tx.send_kind(
            Message::Text("slide 2".into()),
            OutgoingKind::PresenterSlide,
        )
        .expect("queue open");
        tx.send(Message::Text("pong".into())).expect("queue open");
        drop(tx);
        let mut sent = Vec::new();
        let mut dropped = 0;
        while let Some((item, count)) = next(&mut rx, &mut pending).await {
            sent.push(item.message);
            dropped += count;
        }
        assert_eq!(dropped, 2, "decks 2 and 3");
        assert_eq!(
            sent,
            [
                Message::Text("deck 4".into()),
                Message::Text("slide 2".into()),
                Message::Text("pong".into()),
            ]
        );
    }
}
