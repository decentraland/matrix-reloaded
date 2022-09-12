use std::cmp::max;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Arc;

use crate::client::{Client, RegisterResult};
use crate::client::{LoginResult, SyncResult};
use crate::configuration::Config;
use crate::events::{SyncEvent, SyncEventsSender, UserNotificationsSender};
use crate::simulation::Context;
use crate::text::get_random_string;
use async_channel::Sender;
use futures::lock::Mutex;
use matrix_sdk::locks::RwLock;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId};
use rand::distributions::Alphanumeric;
use rand::prelude::SliceRandom;
use rand::rngs::StdRng;
use rand::Rng;

#[derive(Clone, Debug)]
pub struct User {
    pub localpart: String,
    client: Client,
    pub state: State,
}

#[derive(Clone, Debug)]
pub enum MessageType {
    Direct,
    Channel,
}

enum SocialAction {
    AddFriend,
    SendMessage(MessageType),
    LogOut,
    UpdateStatus,
    CreateChannel,
    JoinChannel,
    GetChannelMembers,
    None,
}

#[derive(Clone, Debug)]
pub enum State {
    Unauthenticated,
    Unregistered,
    LoggedIn,
    Sync {
        direct_messages: Arc<RwLock<Vec<OwnedRoomId>>>, // available rooms that user can send messages (personal/direct rooms)
        channels: Arc<RwLock<HashSet<OwnedRoomId>>>, // available channels that user can send messages (he joined or created)
        events: Arc<Mutex<Vec<SyncEvent>>>, // recent events to be processed and react, for instance to respond to friends or join rooms
        cancel_sync: Sender<bool>,          // cancel sync task
        ticks_to_live: usize,               // ticks to live
    },
    LoggedOut,
}

impl User {
    pub async fn new(id_number: usize, notifier: SyncEventsSender, config: &Config) -> Self {
        let localpart = get_user_id_localpart(id_number, &config.simulation.execution_id);

        let client = Client::new(notifier, config).await;
        Self {
            localpart,
            client,
            state: State::Unregistered,
        }
    }

    pub async fn act(&mut self, context: &Context) {
        match &self.state {
            State::Unregistered => self.register().await,
            State::Unauthenticated => self.log_in().await,
            State::LoggedIn => self.sync(&context.config, &context.user_notifier).await,
            State::Sync { .. } => self.socialize(context).await,
            State::LoggedOut => self.restart(&context.config).await,
        }
    }

    async fn add_room(&self, room_id: &RoomId) {
        if let State::Sync {
            direct_messages, ..
        } = &self.state
        {
            direct_messages.write().await.push(room_id.to_owned());
        }
    }

    async fn add_channel(&self, room_id: &RoomId) {
        if let State::Sync { channels, .. } = &self.state {
            channels.write().await.insert(room_id.to_owned());
        }
    }

    async fn restart(&mut self, config: &Config) {
        log::debug!("user '{}' act => {}", self.localpart, "RESTART");
        self.client.reset(config).await;
        self.state = State::Unauthenticated;
    }

    async fn log_in(&mut self) {
        log::debug!("user '{}' act => {}", self.localpart, "LOG IN");

        match self.client.login(&self.localpart).await {
            LoginResult::Ok => {
                self.state = State::LoggedIn;
            }
            LoginResult::NotRegistered => {
                log::debug!("user {} not registered", self.localpart);
                self.state = State::Unregistered;
            }
            LoginResult::Failed => {
                log::debug!(
                    "user {} failed to login, maybe retry next time...",
                    self.localpart
                );
            }
        }
    }

    async fn register(&mut self) {
        log::debug!("user '{}' act => {}", self.localpart, "REGISTER");
        match self.client.register(&self.localpart).await {
            RegisterResult::Ok => self.state = State::Unauthenticated,
            RegisterResult::Failed => log::debug!(
                "could not register user {}, will retry next time...",
                self.localpart
            ),
        }
    }

    pub async fn id(&self) -> Option<OwnedUserId> {
        self.client.user_id().await
    }

    async fn sync(&mut self, config: &Config, user_notifier: &UserNotificationsSender) {
        log::debug!("user '{}' act => {}", self.localpart, "SYNC");
        match self.client.sync(user_notifier).await {
            SyncResult::Ok {
                direct_messages,
                invited_rooms,
                cancel_sync,
                channels,
            } => {
                log::debug!(
                    "user '{}' has {} dm rooms",
                    self.localpart,
                    direct_messages.len()
                );
                log::debug!("user '{}' has {} channels", self.localpart, channels.len());
                log::debug!(
                    "user '{}' has been invited to {} rooms",
                    self.localpart,
                    &invited_rooms.len()
                );

                let mut events = vec![];
                for invited_room in invited_rooms {
                    events.push(SyncEvent::Invite(invited_room));
                }

                for dm_room in &direct_messages {
                    events.push(SyncEvent::UnreadRoom(dm_room.clone()));
                }

                let channels = HashSet::from_iter(channels.iter().cloned());

                let ticks_to_live = get_ticks_to_live(config);
                self.state = State::Sync {
                    direct_messages: Arc::new(RwLock::new(direct_messages)),
                    events: Arc::new(Mutex::new(events)),
                    channels: Arc::new(RwLock::new(channels)),
                    cancel_sync,
                    ticks_to_live,
                };
                log::debug!("user '{}' now is syncing", self.localpart);
            }
            SyncResult::Failed => log::debug!(
                "user {} couldn't make initial sync, will retry next time...",
                self.localpart
            ),
        }
    }

    async fn read_sync_events(&self, events: &Mutex<Vec<SyncEvent>>) {
        log::debug!("user '{}' reading sync events", self.localpart);
        let new_events = self.client.read_sync_events().await;
        let mut events = events.lock().await;
        for event in new_events {
            match event {
                SyncEvent::RoomCreated(room_id) => self.add_room(&room_id).await,
                SyncEvent::ChannelCreated(room_id) => self.add_channel(&room_id).await,
                _ => events.push(event),
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
        log::debug!("user '{}' act => {}", self.localpart, "SOCIALIZE");

        self.decrease_ticks_to_live();
        if let State::Sync {
            direct_messages,
            events,
            cancel_sync,
            ticks_to_live,
            channels,
        } = &self.state
        {
            self.read_sync_events(events).await;
            let mut events = events.lock().await;
            if let Some(event) = events.pop() {
                log::debug!("--- user '{}' going to react", self.localpart);
                self.react(event).await
            } else {
                drop(events);

                log::debug!("--- user '{}' going to start interaction", self.localpart);
                if ticks_to_live <= &0 {
                    // it's time to log out
                    self.log_out(cancel_sync.clone()).await;
                } else {
                    match pick_random_action(
                        context.config.simulation.probability_to_act,
                        context.config.simulation.channels_load,
                        context.config.simulation.allow_get_channel_members,
                    ) {
                        SocialAction::SendMessage(message_type) => match message_type {
                            MessageType::Direct => {
                                self.send_message(
                                    pick_random_room(direct_messages).await,
                                    message_type,
                                )
                                .await
                            }
                            MessageType::Channel => {
                                self.send_message(
                                    pick_random_channels(channels).await,
                                    message_type,
                                )
                                .await
                            }
                        },
                        SocialAction::AddFriend => self.add_friend(context).await,
                        SocialAction::LogOut => self.log_out(cancel_sync.clone()).await,
                        SocialAction::UpdateStatus => self.update_status().await,
                        SocialAction::CreateChannel => {
                            self.create_channel(
                                channels.read().await.len(),
                                context.config.simulation.channels_per_user,
                            )
                            .await
                        }
                        SocialAction::JoinChannel => self.join_channel(context).await,
                        SocialAction::GetChannelMembers => {
                            let channel_id = pick_random_channels(channels).await;
                            if let Some(channel_id) = channel_id {
                                self.get_channel_members(channel_id).await;
                            }
                        }
                        SocialAction::None => log::debug!("user {} did nothing", self.localpart),
                    };
                }
            }
        } else {
            log::debug!("user cannot socialize if is not in sync state!");
        }
    }

    fn decrease_ticks_to_live(&mut self) {
        if let State::Sync { ticks_to_live, .. } = &mut self.state {
            *ticks_to_live -= 1;
        }
    }
    async fn react(&self, event: SyncEvent) {
        log::debug!("user '{}' act => {}", self.localpart, "REACT");
        match event {
            SyncEvent::Invite(room_id) => self.join(&room_id, MessageType::Direct, false).await,
            SyncEvent::MessageReceived(room_id, _, message_type) => {
                self.respond(room_id, message_type).await
            }
            SyncEvent::UnreadRoom(room_id) => self.read_messages(room_id).await,
            SyncEvent::GetChannelMembers(room_id) => self.get_channel_members(room_id).await,
            _ => {}
        }
    }

    async fn read_messages(&self, room_id: OwnedRoomId) {
        log::debug!("user '{}' act => {}", self.localpart, "READ MESSAGES");
        self.client.read_messages(room_id).await;
    }

    async fn get_channel_members(&self, room_id: OwnedRoomId) {
        log::debug!("user '{}' act => {}", self.localpart, "GET CHANNEL MEMBERS");
        self.client.get_channel_members(&room_id).await
    }

    async fn respond(&self, room: OwnedRoomId, message_type: MessageType) {
        match message_type {
            MessageType::Direct => log::debug!(
                "user '{}' act => {}",
                self.localpart,
                "RESPOND DIRECT MESSAGE"
            ),
            MessageType::Channel => {
                log::debug!("user '{}' act => {}", self.localpart, "RESPOND CHANNEL")
            }
        }
        self.send_message(Some(room), message_type).await;
    }

    async fn add_friend(&self, context: &Context) {
        log::debug!("user '{}' act => {}", self.localpart, "ADD FRIEND");
        let friend_id = self.pick_friend(context);
        if let Some(friend_id) = friend_id {
            self.client.add_friend(&friend_id).await;
        } else {
            log::debug!("there are no users to add as friend :(");
        }
    }

    async fn create_channel(&self, current_user_channels: usize, channels_per_user: usize) {
        if current_user_channels < channels_per_user {
            let channel_name: String = rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(7)
                .map(char::from)
                .collect();
            log::debug!(
                "user '{}' act => {} => {}",
                self.localpart,
                "CREATE CHANNEL",
                channel_name
            );
            self.client.create_channel(channel_name).await
        } else {
            log::debug!(
                "user '{}' act => {} per user: {}, current user: {}",
                self.localpart,
                "REACH CHANNEL LIMIT CREATION",
                channels_per_user,
                current_user_channels
            )
        }
    }

    async fn join_channel(&self, context: &Context) {
        log::debug!("user '{}' act => {}", self.localpart, "JOIN CHANNEL");
        let user_channels = match &self.state {
            State::Sync { channels, .. } => channels,
            _ => {
                log::debug!("user '{}' was not synced", self.localpart);
                return;
            }
        };
        let ctx_channels = context.channels.read().await;
        let user_channels = user_channels.read().await;
        let exclude_user_channels = ctx_channels.difference(&user_channels).collect::<Vec<_>>();
        if !exclude_user_channels.is_empty()
            && user_channels.len() < context.config.simulation.channels_per_user
        {
            // we use this type of Random because it can be used in threads
            // check out: https://docs.rs/rand/0.5.0/rand/rngs/struct.StdRng.html
            // or https://stackoverflow.com/questions/65053037/how-can-i-clone-a-random-number-generator-for-different-threads-in-rust
            let mut rng: StdRng = rand::SeedableRng::from_entropy();
            loop {
                let channel = exclude_user_channels.choose(&mut rng);
                if let Some(room_id) = channel {
                    self.join(
                        room_id,
                        MessageType::Channel,
                        context.config.simulation.allow_get_channel_members,
                    )
                    .await;
                    log::debug!(
                        "user '{}' act => {} {}",
                        self.localpart,
                        "JOINED CHANNEL",
                        room_id
                    );
                    break;
                }
            }
        }
    }

    async fn join(&self, room: &RoomId, room_type: MessageType, allow_get_channel_members: bool) {
        match room_type {
            MessageType::Direct => log::debug!("user '{}' act => {}", self.localpart, "JOIN ROOM"),
            MessageType::Channel => {
                log::debug!("user '{}' act => {}", self.localpart, "JOIN CHANNEL")
            }
        }
        self.client
            .join_room(room, room_type, allow_get_channel_members)
            .await;
    }

    async fn send_message(&self, room: Option<OwnedRoomId>, message_type: MessageType) {
        match message_type {
            MessageType::Direct => {
                log::debug!("user '{}' act => {}", self.localpart, "SEND DIRECT MESSAGE")
            }
            MessageType::Channel => log::debug!(
                "user '{}' act => {}",
                self.localpart,
                "SEND CHANNEL MESSAGE"
            ),
        }
        if let Some(room) = room {
            self.client.send_message(&room, get_random_string()).await;
        } else {
            log::debug!("trying to send message to friend but don't have one :(")
        }
    }

    /// Log out user and append new char to the localpart string so next iteration is a new user.
    async fn log_out(&mut self, cancel_sync: Sender<bool>) {
        log::debug!("user '{}' act => {}", self.localpart, "LOG OUT");
        cancel_sync.send(true).await.expect("channel open");
        self.state = State::LoggedOut;
        self.localpart += "_";
    }

    async fn update_status(&self) {
        log::debug!("user '{}' act => {}", self.localpart, "UPDATE STATUS");
        self.client.update_status().await;
    }

    fn pick_friend(&self, context: &Context) -> Option<OwnedUserId> {
        let mut rng = rand::thread_rng();
        loop {
            let friend_id = context.syncing_users.choose(&mut rng)?;
            if friend_id.localpart() != self.localpart {
                return Some(friend_id.clone());
            }
        }
    }
}

fn get_user_id_localpart(id_number: usize, execution_id: &str) -> String {
    format!("user_{id_number}_{execution_id}")
}

// we probably want to distribute these actions and don't make them random (more send messages than logouts)
fn pick_random_action(
    probability_to_act: usize,
    channels_enabled: bool,
    allow_get_channel_members: bool,
) -> SocialAction {
    let mut rng = rand::thread_rng();
    if rng.gen_ratio(probability_to_act as u32, 100) {
        if rng.gen_ratio(1, 75) {
            SocialAction::LogOut
        } else if channels_enabled && allow_get_channel_members && rng.gen_ratio(1, 60) {
            SocialAction::GetChannelMembers
        } else if channels_enabled && rng.gen_ratio(1, 50) {
            SocialAction::CreateChannel
        } else if channels_enabled && rng.gen_ratio(1, 35) {
            SocialAction::JoinChannel
        } else if rng.gen_ratio(1, 25) {
            SocialAction::UpdateStatus
        } else if rng.gen_ratio(1, 3) {
            SocialAction::AddFriend
        } else if channels_enabled && rng.gen_ratio(1, 5) {
            SocialAction::SendMessage(MessageType::Channel)
        } else {
            SocialAction::SendMessage(MessageType::Direct)
        }
    } else {
        SocialAction::None
    }
}

async fn pick_random_room(rooms: &RwLock<Vec<OwnedRoomId>>) -> Option<OwnedRoomId> {
    rooms
        .read()
        .await
        .choose(&mut rand::thread_rng())
        .map(|room| room.to_owned())
}

async fn pick_random_channels(channels: &RwLock<HashSet<OwnedRoomId>>) -> Option<OwnedRoomId> {
    let channels = channels.read().await;
    let channels_vec = channels.iter().collect::<Vec<_>>();
    channels_vec
        .choose(&mut rand::thread_rng())
        .map(|room| room.to_owned().to_owned())
}

/// Get random value for ticks to live related to the total of ticks in simulation,
/// so users can be short or long lived.
fn get_ticks_to_live(config: &Config) -> usize {
    let mut rng = rand::thread_rng();
    let short_lived = rng.gen_bool(config.simulation.probability_for_short_lifes as f64 / 100.);
    match short_lived {
        true => max(config.simulation.ticks / 100, 5),
        false => max(config.simulation.ticks / 10, 10),
    }
}
