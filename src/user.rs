use std::sync::Arc;

use crate::client::{Client, RegisterResult};
use crate::client::{LoginResult, SyncResult};
use crate::configuration::{get_homeserver_url, Config};
use crate::events::{Notifier, SyncEvent};
use crate::simulation::Context;
use crate::text::get_random_string;
use async_channel::Sender;
use futures::lock::Mutex;
use matrix_sdk::locks::RwLock;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId, UserId};
use rand::prelude::SliceRandom;
use rand::Rng;

#[derive(Clone, Debug)]
pub struct User {
    pub id: OwnedUserId,
    client: Client,
    pub state: State,
}

#[derive(Clone, Debug)]
pub enum State {
    Unauthenticated,
    Unregistered,
    LoggedIn,
    Sync {
        rooms: Arc<RwLock<Vec<OwnedRoomId>>>, // available rooms that user can send messages
        events: Arc<Mutex<Vec<SyncEvent>>>, // recent events to be processed and react, for instance to respond to friends or join rooms
        cancel_sync: Sender<bool>,          // cancel sync task
    },
    LoggedOut,
}

impl User {
    pub async fn new(id_number: usize, notifier: Notifier, config: &Config) -> Self {
        let (homeserver, _) = get_homeserver_url(&config.server.homeserver, None);
        let id = get_user_id(
            id_number,
            homeserver.as_str(),
            &config.simulation.execution_id,
        );

        Self {
            id,
            client: Client::new(notifier, config).await,
            state: State::Unauthenticated,
        }
    }

    pub async fn act(&mut self, context: &Context) {
        match &self.state {
            State::Unauthenticated => self.log_in().await,
            State::Unregistered => self.register().await,
            State::LoggedIn => self.sync().await,
            State::Sync { .. } => self.socialize(context).await,
            State::LoggedOut => self.restart(&context.config).await,
        }
    }

    async fn add_room(&self, room_id: &RoomId) {
        if let State::Sync { rooms, .. } = &self.state {
            rooms.write().await.push(room_id.to_owned());
        }
    }

    async fn restart(&mut self, config: &Config) {
        log::debug!("user '{}' act => {}", self.id, "RESTART");
        self.client.reset(config).await;
        self.state = State::Unauthenticated;
    }

    async fn log_in(&mut self) {
        log::debug!("user '{}' act => {}", self.id, "LOG IN");

        match self.client.login(&self.id).await {
            LoginResult::Ok => {
                self.state = State::LoggedIn;
            }
            LoginResult::NotRegistered => {
                log::debug!("user {} not registered", self.id);
                self.state = State::Unregistered;
            }
            LoginResult::Failed => {
                log::debug!("user {} failed to login, maybe retry next time...", self.id);
            }
        }
    }

    async fn register(&mut self) {
        log::debug!("user '{}' act => {}", self.id, "REGISTER");
        match self.client.register(&self.id).await {
            RegisterResult::Ok => self.state = State::Unauthenticated,
            RegisterResult::Failed => log::debug!(
                "could not register user {}, will retry next time...",
                self.id
            ),
        }
    }

    async fn sync(&mut self) {
        log::debug!("user '{}' act => {}", self.id, "SYNC");
        match self.client.sync(&self.id).await {
            SyncResult::Ok {
                joined_rooms,
                invited_rooms,
                cancel_sync,
            } => {
                log::debug!("user '{}' has {} rooms", self.id, joined_rooms.len());
                log::debug!(
                    "user '{}' has been invited to {} rooms",
                    self.id,
                    &invited_rooms.len()
                );

                let mut events = vec![];
                for invited_room in invited_rooms {
                    events.push(SyncEvent::Invite(invited_room));
                }

                self.state = State::Sync {
                    rooms: Arc::new(RwLock::new(joined_rooms)),
                    events: Arc::new(Mutex::new(events)),
                    cancel_sync,
                };
                log::debug!("user '{}' now is syncing", self.id);
            }
            SyncResult::Failed => log::debug!(
                "user {} couldn't make initial sync, will retry next time...",
                self.id
            ),
        }
    }

    async fn read_sync_events(&self, events: &Mutex<Vec<SyncEvent>>) {
        log::debug!("user '{}' reading sync events", self.id);
        let new_events = self.client.read_sync_events().await;
        let mut events = events.lock().await;
        for event in new_events {
            if let SyncEvent::RoomCreated(room_id) = event {
                self.add_room(&room_id).await;
            } else {
                events.push(event);
            }
        }
    }

    // user social skills are:
    // - react to received messages or invitations
    // - send a message to a friend
    // - add a new friend
    // - update status
    // - log out (not so social)
    async fn socialize(&mut self, context: &Context) {
        log::debug!("user '{}' act => {}", self.id, "SOCIALIZE");

        if let State::Sync {
            rooms,
            events,
            cancel_sync,
        } = &self.state
        {
            self.read_sync_events(events).await;
            let mut events = events.lock().await;
            if let Some(event) = events.pop() {
                log::debug!("--- user '{}' going to react", self.id);
                self.react(event).await
            } else {
                drop(events);

                log::debug!("--- user '{}' going to start interaction", self.id);
                match pick_random_action() {
                    SocialAction::SendMessage => {
                        self.send_message(pick_random_room(rooms).await).await
                    }
                    SocialAction::AddFriend => self.add_friend(context).await,
                    SocialAction::LogOut => self.log_out(cancel_sync.clone()).await,
                    SocialAction::UpdateStatus => self.update_status().await,
                };
            }
        } else {
            log::debug!("user cannot socialize if is not in sync state!");
        }
    }

    async fn react(&self, event: SyncEvent) {
        log::debug!("user '{}' act => {}", self.id, "REACT");
        match event {
            SyncEvent::Invite(room_id) => self.join(&room_id).await,
            SyncEvent::Message(room_id, _) => self.respond(room_id).await,
            _ => {}
        }
    }

    async fn respond(&self, room: OwnedRoomId) {
        log::debug!("user '{}' act => {}", self.id, "RESPOND");
        self.send_message(Some(room)).await;
    }

    async fn add_friend(&self, context: &Context) {
        log::debug!("user '{}' act => {}", self.id, "ADD FRIEND");
        let friend_id = self.pick_friend(context);
        if let Some(friend_id) = friend_id {
            self.client.add_friend(&friend_id).await;
        } else {
            log::debug!("there are no users to add as friend :(");
        }
    }

    async fn join(&self, room: &RoomId) {
        log::debug!("user '{}' act => {}", self.id, "JOIN ROOM");
        self.client.join_room(room).await;
    }

    async fn send_message(&self, room: Option<OwnedRoomId>) {
        log::debug!("user '{}' act => {}", self.id, "SEND MESSAGE");
        if let Some(room) = room {
            self.client.send_message(&room, get_random_string()).await;
        } else {
            log::debug!("trying to send message to friend but don't have one :(")
        }
    }

    async fn log_out(&mut self, cancel_sync: Sender<bool>) {
        log::debug!("user '{}' act => {}", self.id, "LOG OUT");
        cancel_sync.send(true).await.expect("channel open");
        self.state = State::LoggedOut;
    }

    async fn update_status(&self) {
        log::debug!("user '{}' act => {}", self.id, "UPDATE STATUS");
        self.client.update_status(&self.id).await;
    }

    fn pick_friend(&self, context: &Context) -> Option<OwnedUserId> {
        let mut rng = rand::thread_rng();
        let (homeserver, _) = get_homeserver_url(&context.config.server.homeserver, None);
        loop {
            let id_number = context.syncing_users.choose(&mut rng)?;
            let friend_id = get_user_id(
                *id_number,
                homeserver.as_str(),
                &context.config.simulation.execution_id,
            );
            if friend_id != self.id {
                return Some(friend_id);
            }
        }
    }
}

fn get_user_id(id_number: usize, homeserver: &str, execution_id: &str) -> OwnedUserId {
    <&UserId>::try_from(format!("@user_{id_number}_{execution_id}:{homeserver}").as_str())
        .unwrap()
        .to_owned()
}

enum SocialAction {
    AddFriend,
    SendMessage,
    LogOut,
    UpdateStatus,
}

// we probably want to distribute these actions and don't make them random (more send messages than logouts)
fn pick_random_action() -> SocialAction {
    let mut rng = rand::thread_rng();
    if rng.gen_ratio(1, 50) {
        SocialAction::LogOut
    } else if rng.gen_ratio(1, 25) {
        SocialAction::UpdateStatus
    } else if rng.gen_ratio(1, 3) {
        SocialAction::AddFriend
    } else {
        SocialAction::SendMessage
    }
}

async fn pick_random_room(rooms: &RwLock<Vec<OwnedRoomId>>) -> Option<OwnedRoomId> {
    rooms
        .read()
        .await
        .choose(&mut rand::thread_rng())
        .map(|room| room.to_owned())
}
