use matrix_sdk::instant::Instant;
use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageEventContent, SyncMessageEvent};
use matrix_sdk::{Client, ClientConfig, RequestConfig, SyncSettings};
use rand::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use matrix_sdk::ruma::{assign, RoomId, UserId};

use crate::metrics::Metrics;
use crate::text::get_random_string;

const PASSWORD: &str = "asdfasdf";

pub struct Disconnected {
    metrics: Metrics,
}
pub struct Registered {
    metrics: Metrics,
}
pub struct LoggedIn {
    metrics: Metrics,
}
#[derive(Default, Clone)]
pub struct Synching {
    rooms: Arc<Mutex<Vec<RoomId>>>,
    metrics: Metrics,
}

#[derive(Clone)]
pub struct User<State> {
    id: UserId,
    client: Arc<tokio::sync::Mutex<Client>>,
    state: State,
}

impl<State> User<State> {
    pub fn id(&self) -> UserId {
        self.id.clone()
    }
}

#[derive(Serialize, Debug, Eq, Hash, PartialEq, Clone)]
pub enum UserRequest {
    Register,
    Login,
    CreateRoom,
    JoinRoom,
    SendMessage,
}

impl User<Disconnected> {
    pub async fn new(
        id: &str,
        homeserver: &str,
        retry_enabled: bool,
        metrics: Metrics,
    ) -> Option<User<Disconnected>> {
        // init client connection (register + login)
        let user_id = UserId::try_from(format!("@{id}:{homeserver}")).unwrap();

        let instant = Instant::now();

        // in case we are testing against localhost or not https server, we need to setup test cfg, see `Client::homeserver_from_user_id`
        let config = if retry_enabled {
            ClientConfig::new()
                .request_config(RequestConfig::new().retry_timeout(Duration::from_secs(30)))
        } else {
            ClientConfig::new().request_config(
                RequestConfig::new()
                    .disable_retry()
                    .timeout(Duration::from_secs(30)),
            )
        };
        let client = Client::new_from_user_id_with_config(user_id.clone(), config).await;
        if client.is_err() {
            return None;
        }

        log::info!("new client {} {}", user_id, instant.elapsed().as_millis());

        Some(Self {
            id: user_id,
            client: Arc::new(tokio::sync::Mutex::new(client.unwrap())),
            state: Disconnected { metrics },
        })
    }

    pub async fn register(&mut self) -> Option<User<Registered>> {
        let instant = Instant::now();

        let req = assign!(register::Request::new(), {
            username: Some(self.id.localpart()),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });
        let client = self.client.lock().await;
        let response = client.register(req).await;

        match response {
            Ok(_) => {
                self.state
                    .metrics
                    .report_request_duration((UserRequest::Register, instant.elapsed()));
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    state: Registered {
                        metrics: self.state.metrics.clone(),
                    },
                })
            }
            Err(e) => {
                self.state.metrics.report_error((e, UserRequest::Register));
                None
            }
        }
    }
}

impl User<Registered> {
    pub async fn login(&mut self) -> Option<User<LoggedIn>> {
        let instant = Instant::now();

        let client = self.client.lock().await;
        let response = client
            .login(self.id.localpart(), PASSWORD, None, None)
            .await;

        match response {
            Ok(_) => {
                self.state
                    .metrics
                    .report_request_duration((UserRequest::Login, instant.elapsed()));
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    state: LoggedIn {
                        metrics: self.state.metrics.clone(),
                    },
                })
            }
            Err(e) => {
                if let matrix_sdk::Error::Http(e) = e {
                    self.state.metrics.report_error((e, UserRequest::Login));
                }

                None
            }
        }
    }
}

impl User<LoggedIn> {
    pub async fn sync(&self) -> User<Synching> {
        let client = self.client.lock().await;
        client
            .register_event_handler({
                let metrics = self.state.metrics.clone();
                let user_id = self.id.clone();
                move |ev: SyncMessageEvent<MessageEventContent>, room: Room| {
                    let mut metrics = metrics.clone();
                    let user_id = user_id.clone();
                    async move {
                        if ev.sender.localpart() == user_id.localpart() {
                            return;
                        }
                        log::info!(
                            "User {} received a message from room {} and sent by {}",
                            user_id,
                            room.room_id(),
                            ev.sender
                        );
                        metrics.report_message_received(ev.event_id.to_string());
                    }
                }
            })
            .await;

        tokio::spawn({
            // we are not cloning the mutex to avoid locking it forever
            let client = client.clone();
            async move {
                client.sync(SyncSettings::default()).await;
            }
        });

        User {
            id: self.id.clone(),
            client: self.client.clone(),
            state: Synching {
                metrics: self.state.metrics.clone(),
                rooms: Arc::new(Mutex::new(vec![])),
            },
        }
    }
}

impl User<Synching> {
    pub async fn create_room(&mut self) -> Option<RoomId> {
        let client = self.client.lock().await;

        let instant = Instant::now();
        let request = create_room::Request::new();
        let response = client.create_room(request).await;
        match response {
            Ok(ref response) => {
                self.state
                    .metrics
                    .report_request_duration((UserRequest::CreateRoom, instant.elapsed()));
                Some(response.room_id.clone())
            }
            Err(e) => {
                self.state
                    .metrics
                    .report_error((e, UserRequest::CreateRoom));
                None
            }
        }
    }

    pub async fn join_room(&mut self, room_id: &RoomId) {
        let client = self.client.lock().await;
        let instant = Instant::now();
        let response = client.join_room_by_id(room_id).await;
        match response {
            Ok(ref response) => {
                self.state
                    .metrics
                    .report_request_duration((UserRequest::JoinRoom, instant.elapsed()));
                self.state.rooms.lock().await.push(response.room_id.clone());
            }
            Err(e) => {
                self.state.metrics.report_error((e, UserRequest::JoinRoom));
            }
        }
    }

    pub async fn act(&mut self) {
        let client = self.client.lock().await;
        let rooms = self.state.rooms.lock().await;

        let room = match rooms.len() {
            0 => {
                // if user is not present in any room, return
                return;
            }
            1 => 0,
            rooms_len => rand::thread_rng().gen_range(0..rooms_len),
        };

        let room_id = &rooms[room];
        let content = AnyMessageEventContent::RoomMessage(MessageEventContent::text_plain(
            get_random_string(),
        ));
        let instant = Instant::now();
        let response = client.room_send(room_id, content, None).await;

        match response {
            Ok(response) => {
                self.state
                    .metrics
                    .report_request_duration((UserRequest::SendMessage, instant.elapsed()));

                self.state
                    .metrics
                    .report_message_sent(response.event_id.to_string());
            }
            Err(e) => {
                if let matrix_sdk::Error::Http(e) = e {
                    self.state
                        .metrics
                        .report_error((e, UserRequest::SendMessage));
                }
            }
        }
    }
}
