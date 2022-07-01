use matrix_sdk::{ruma::OwnedRoomId, HttpError};
use serde::Serialize;
use std::time::Duration;
use strum::Display;
use tokio::sync::mpsc::Sender;

pub type Notifier = Sender<Event>;

#[derive(Serialize, Debug, Eq, Hash, PartialEq, Clone, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum UserRequest {
    Register,
    Login,
    CreateRoom,
    JoinRoom,
    SendMessage,
    UpdateAccountData,
    Presence,
}

#[derive(Debug)]
pub enum Event {
    MessageSent(String),
    MessageReceived(String),
    RequestDuration((UserRequest, Duration)),
    Error((UserRequest, HttpError)),
}

#[derive(Clone, Debug)]
pub enum SyncEvent {
    Invite(OwnedRoomId),
    Message(OwnedRoomId, String),
}
