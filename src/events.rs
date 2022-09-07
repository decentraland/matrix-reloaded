use crate::report::Report;
use matrix_sdk::locks::RwLock;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::HttpError;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, time::Instant};
use strum::Display;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

pub type Notifier = Sender<Event>;
pub type UserNotifier = Sender<UserNotifications>;

#[derive(Serialize, Debug, Eq, Hash, PartialEq, Clone, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum UserRequest {
    Register,
    Login,
    InitialSync,
    CreateRoom,
    JoinRoom,
    SendMessage,
    UpdateStatus,
    Messages,
    CreateChannel,
}

#[derive(Debug)]
pub enum UserNotifications {
    NewChannel(OwnedRoomId),
}

#[derive(Debug)]
pub enum Event {
    MessageSent(String),
    MessageReceived(String),
    RequestDuration((UserRequest, Duration)),
    Error((UserRequest, HttpError)),
    Finish,
}

#[derive(Clone, Debug)]
pub enum SyncEvent {
    Invite(OwnedRoomId),
    RoomCreated(OwnedRoomId),
    UnreadRoom(OwnedRoomId),
    MessageReceived(OwnedRoomId, String),
    ChannelCreated(OwnedRoomId),
}

#[derive(Default)]
pub struct MessageTimes {
    pub sent: Option<Instant>,
    pub received: Option<Instant>,
}

pub struct EventCollector {
    events: Arc<Events>,
}

#[derive(Default)]
struct Events {
    requests: RwLock<Vec<(UserRequest, Duration)>>,
    errors: RwLock<Vec<(UserRequest, HttpError)>>,
    messages: RwLock<HashMap<String, MessageTimes>>,
}

impl Events {
    async fn report(&self) -> Report {
        let errors = self.errors.read().await;
        let requests = self.requests.read().await;
        let messages = self.messages.read().await;

        Report::from(&errors, &requests, &messages)
    }
}

impl EventCollector {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Events::default()),
        }
    }

    pub fn start(&self, receiver: Receiver<Event>) -> JoinHandle<Report> {
        tokio::spawn(Self::collect_events(receiver, self.events.clone()))
    }

    ///
    /// # Panics
    /// If message sent event is processed and the message_id is already present in the messages map
    /// If message received event is processed  and the message_id is not present in the messages map
    ///
    async fn collect_events(mut receiver: Receiver<Event>, events: Arc<Events>) -> Report {
        while let Some(event) = receiver.recv().await {
            log::debug!("Event received {:?}", event);
            match event {
                Event::Error(e) => {
                    events.errors.write().await.push(e);
                }
                Event::MessageSent(message_id) => {
                    let mut messages = events.messages.write().await;
                    messages.entry(message_id).or_default().sent = Some(Instant::now());
                }
                Event::MessageReceived(message_id) => {
                    let mut messages = events.messages.write().await;
                    messages.entry(message_id).or_default().received = Some(Instant::now());
                }
                Event::RequestDuration(request) => {
                    events.requests.write().await.push(request);
                }
                Event::Finish => break,
            }
        }

        log::debug!("couldn't read event or simulation finished");
        receiver.close();

        events.report().await
    }
}
