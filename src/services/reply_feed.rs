// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    convert::Infallible,
    fmt::Write,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use axum::response::sse::Event;
use futures_util::{Stream, StreamExt};
use sea_orm::EntityTrait;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{conversations, messages},
    services::conversation_access::can_access_conversation,
    state::SharedState,
};

// Bound the memory one reply's replay can hold. A larger reply stops recording, and a page that
// reconnects to it falls back to reading the saved conversation.
const MAX_EVENTS: usize = 20_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;
// How long a finished reply's events stay for followers still replaying them. After that they
// are freed even if a stalled connection still holds the feed.
const KEEP_AFTER_CLOSE: Duration = Duration::from_secs(30);

// Every event of one reply's stream, kept while the reply generates, so a page that lost the
// stream (a refresh) can replay it from the start or from the last event it saw, then follow it.
#[derive(Default)]
pub struct ReplyFeed {
    state: Mutex<FeedState>,
    changed: Notify,
}

#[derive(Default)]
struct FeedState {
    events: Vec<Event>,
    bytes: usize,
    closed: bool,
    discarded: bool,
}

impl FeedState {
    fn discard(&mut self) {
        self.discarded = true;
        self.events = Vec::new();
        self.bytes = 0;
    }
}

#[derive(Default)]
struct ByteCount(usize);

impl Write for ByteCount {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0 += text.len();
        Ok(())
    }
}

// axum does not expose an event's encoded length; its Debug form escapes the encoded bytes, so
// it is never shorter and serves as an upper bound.
fn event_size(event: &Event) -> usize {
    let mut count = ByteCount::default();
    let _ = write!(count, "{event:?}");
    count.0
}

enum Next {
    Event(Event),
    Wait,
    End,
}

impl ReplyFeed {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn state(&self) -> MutexGuard<'_, FeedState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push(&self, event: Event) {
        let size = event_size(&event);
        {
            let mut state = self.state();
            if state.closed || state.discarded {
                return;
            }
            if state.events.len() >= MAX_EVENTS || state.bytes + size > MAX_BYTES {
                state.discard();
            } else {
                state.bytes += size;
                state.events.push(event);
            }
        }
        self.changed.notify_waiters();
    }

    fn close(&self) {
        self.state().closed = true;
        self.changed.notify_waiters();
    }

    fn discard(&self) {
        self.state().discard();
        self.changed.notify_waiters();
    }

    fn resumable(&self) -> bool {
        !self.state().discarded
    }

    fn next(&self, position: usize) -> Next {
        let state = self.state();
        if state.discarded {
            Next::End
        } else if let Some(event) = state.events.get(position) {
            Next::Event(event.clone())
        } else if state.closed {
            Next::End
        } else {
            Next::Wait
        }
    }

    // The events after the first `after`, each carrying its 1-based position as the SSE id so a
    // client can resume with Last-Event-ID, then the new ones as they arrive until the reply ends.
    pub fn follow(self: Arc<Self>, after: usize) -> impl Stream<Item = Result<Event, Infallible>> {
        futures_util::stream::unfold((self, after), |(feed, position)| async move {
            loop {
                // Registered before checking, so an event pushed in between still wakes us.
                let waiter = feed.clone();
                let changed = waiter.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                match feed.next(position) {
                    Next::Event(event) => {
                        let event = event.id((position + 1).to_string());
                        return Some((Ok(event), (feed, position + 1)));
                    }
                    Next::End => return None,
                    Next::Wait => changed.await,
                }
            }
        })
    }
}

struct CloseOnDrop(Arc<ReplyFeed>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
        let feed = self.0.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    tokio::time::sleep(KEEP_AFTER_CLOSE).await;
                    feed.discard();
                });
            }
            Err(_) => feed.discard(),
        }
    }
}

// Copies every event of a reply's stream into its feed. The feed closes when the stream ends,
// or is dropped because the reply's task stopped, so a follower never waits forever.
pub fn record<S>(feed: Arc<ReplyFeed>, events: S) -> impl Stream<Item = Result<Event, Infallible>>
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    async_stream::stream! {
        let _close = CloseOnDrop(feed.clone());
        let mut events = std::pin::pin!(events);
        while let Some(event) = events.next().await {
            if let Ok(event) = &event {
                feed.push(event.clone());
            }
            yield event;
        }
    }
}

// The feed of a reply the user may follow: their own conversation's reply that is still
// generating on this instance. Anything else is reported as not found, and the client reads the
// saved conversation instead.
pub async fn resumable_reply(
    state: &SharedState,
    user_id: Uuid,
    message_id: Uuid,
) -> Result<Arc<ReplyFeed>, AppError> {
    let message = messages::Entity::find_by_id(message_id)
        .one(&state.database)
        .await
        .map_err(|error| {
            eprintln!("resume stream message lookup error: {error}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;
    let conversation = conversations::Entity::find_by_id(message.conversation_id)
        .one(&state.database)
        .await
        .map_err(|error| {
            eprintln!("resume stream conversation lookup error: {error}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;
    if !can_access_conversation(conversation.user_id, user_id) {
        return Err(AppError::ResourceNotFound);
    }
    state
        .reply_feed(message_id)
        .await
        .filter(|feed| feed.resumable())
        .ok_or(AppError::ResourceNotFound)
}

// Last-Event-ID wins over the explicit `after` parameter.
pub fn resume_position(last_event_id: Option<&str>, after: Option<u64>) -> usize {
    last_event_id
        .and_then(|id| id.trim().parse::<u64>().ok())
        .or(after)
        .and_then(|position| usize::try_from(position).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(event: &Event) -> String {
        format!("{event:?}")
    }

    async fn collect(stream: impl Stream<Item = Result<Event, Infallible>>) -> Vec<String> {
        stream.map(|event| data(&event.unwrap())).collect().await
    }

    #[tokio::test]
    async fn a_follower_replays_from_its_position_with_ids_then_ends_with_the_reply() {
        let feed = ReplyFeed::new();
        let reply = futures_util::stream::iter(
            ["one", "two", "three"].map(|text| Ok::<_, Infallible>(Event::default().data(text))),
        );
        let delivered = collect(record(feed.clone(), reply)).await;
        assert_eq!(delivered.len(), 3);

        let all = collect(feed.clone().follow(0)).await;
        assert_eq!(all.len(), 3);
        assert!(all[0].contains("data: one") && all[0].contains("id: 1"));
        let rest = collect(feed.clone().follow(2)).await;
        assert_eq!(rest.len(), 1);
        assert!(rest[0].contains("data: three") && rest[0].contains("id: 3"));
        assert!(collect(feed.follow(5)).await.is_empty());
    }

    #[tokio::test]
    async fn a_follower_receives_live_events_and_stops_when_the_reply_task_drops() {
        let feed = ReplyFeed::new();
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);
        let reply = futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        });
        let producer = tokio::spawn(record(feed.clone(), reply).for_each(|_| async {}));
        let follower = tokio::spawn(collect(feed.clone().follow(0)));
        sender
            .send(Ok(Event::default().data("live")))
            .await
            .unwrap();
        while matches!(feed.next(0), Next::Wait) {
            tokio::task::yield_now().await;
        }
        producer.abort();
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), follower)
            .await
            .expect("the follower ends once the reply task is gone")
            .unwrap();
        assert!(received.iter().any(|event| event.contains("data: live")));
    }

    #[tokio::test]
    async fn a_reply_over_the_byte_limit_is_not_resumable() {
        let feed = ReplyFeed::new();
        let large = "x".repeat(MAX_BYTES / 4);
        for _ in 0..3 {
            feed.push(Event::default().data(large.as_str()));
        }
        assert!(feed.resumable());
        feed.push(Event::default().data(large.as_str()));
        assert!(!feed.resumable());
        assert_eq!(feed.state().bytes, 0);
        assert!(collect(feed.follow(0)).await.is_empty());
    }

    #[test]
    fn an_event_size_is_at_least_its_encoded_length() {
        let event = Event::default().event("delta").data("héllo\nworld");
        assert!(event_size(&event) >= "event: delta\ndata: héllo\ndata: world\n\n".len());
    }

    #[test]
    fn a_finished_reply_frees_its_events_once_no_follower_needs_them() {
        let feed = ReplyFeed::new();
        feed.push(Event::default().data("x"));
        drop(CloseOnDrop(feed.clone()));
        assert!(!feed.resumable());
        assert!(feed.state().events.is_empty());
    }

    #[tokio::test]
    async fn an_oversized_reply_is_not_resumable() {
        let feed = ReplyFeed::new();
        for _ in 0..=MAX_EVENTS {
            feed.push(Event::default().data("x"));
        }
        assert!(!feed.resumable());
        assert!(collect(feed.follow(0)).await.is_empty());
    }

    #[test]
    fn the_resume_position_prefers_last_event_id() {
        assert_eq!(resume_position(Some("7"), Some(3)), 7);
        assert_eq!(resume_position(Some(" 12 "), None), 12);
        assert_eq!(resume_position(Some("bogus"), Some(3)), 3);
        assert_eq!(resume_position(None, None), 0);
    }
}
