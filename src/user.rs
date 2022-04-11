use std::sync::{Arc, Mutex};
use std::time::Duration;

use matrix_sdk::instant::Instant;
use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageEventContent, SyncMessageEvent};
use matrix_sdk::{Client, ClientConfig, RequestConfig, SyncSettings};

use matrix_sdk::ruma::{assign, RoomId, UserId};

use crate::metrics::Metrics;
use crate::text::get_random_string;

const PASSWORD: &str = "asdfasdf";

pub struct Disconnected {
    metrics: Arc<Mutex<Metrics>>,
}
pub struct Registered {
    metrics: Arc<Mutex<Metrics>>,
}
pub struct LoggedIn {
    metrics: Arc<Mutex<Metrics>>,
}
#[derive(Default)]
pub struct Synching {
    rooms: Vec<RoomId>,
    metrics: Arc<Mutex<Metrics>>,
}

pub struct User<State> {
    id: UserId,
    client: Client,
    state: State,
}

impl<State> User<State> {
    pub fn id(&self) -> UserId {
        self.id.clone()
    }
}

impl User<Disconnected> {
    pub async fn new(
        id: String,
        homeserver: String,
        metrics: Arc<Mutex<Metrics>>,
    ) -> Option<User<Disconnected>> {
        // init client connection (register + login)
        let user_id = UserId::try_from(format!("@{id}:{homeserver}")).unwrap();

        let instant = Instant::now();

        // in case we are testing against localhost or not https server, we need to setup test cfg, see `Client::homeserver_from_user_id`
        let client = Client::new_from_user_id_with_config(
            user_id.clone(),
            ClientConfig::new().request_config(
                RequestConfig::new()
                    .disable_retry()
                    .timeout(Duration::from_secs(30)),
            ),
        )
        .await;
        if client.is_err() {
            return None;
        }

        log::info!("new client {} {}", user_id, instant.elapsed().as_millis());

        Some(Self {
            id: user_id,
            client: client.unwrap(),
            state: Disconnected { metrics },
        })
    }

    pub async fn register(&self) -> Option<User<Registered>> {
        let instant = Instant::now();

        let req = assign!(register::Request::new(), {
            username: Some(self.id.localpart()),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });
        let response = self.client.register(req).await;

        match response {
            Ok(_) => {
                log::info!(
                    "registered req {} {}",
                    self.id,
                    instant.elapsed().as_millis()
                );
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    state: Registered {
                        metrics: self.state.metrics.clone(),
                    },
                })
            }
            Err(e) => {
                let mut metrics = self.state.metrics.lock().unwrap();
                metrics.report_error(e);
                None
            }
        }
    }
}

impl User<Registered> {
    pub async fn login(&self) -> Option<User<LoggedIn>> {
        let instant = Instant::now();

        let response = self
            .client
            .login(self.id.localpart(), PASSWORD, None, None)
            .await;

        match response {
            Ok(_) => {
                log::info!(
                    "login new client {} {}",
                    self.id,
                    instant.elapsed().as_millis()
                );
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    state: LoggedIn {
                        metrics: self.state.metrics.clone(),
                    },
                })
            }
            Err(e) => {
                log::info!(
                    "Couldn't log in user {} {}, reason {}",
                    self.id,
                    instant.elapsed().as_millis(),
                    e
                );
                if let matrix_sdk::Error::Http(e) = e {
                    let mut metrics = self.state.metrics.lock().unwrap();
                    metrics.report_error(e);
                }

                None
            }
        }
    }
}

impl User<LoggedIn> {
    pub async fn sync(&self) -> User<Synching> {
        let instant = Instant::now();

        self.client
            .register_event_handler({
                let metrics = self.state.metrics.clone();
                let user_id = self.id.clone();
                move |ev: SyncMessageEvent<MessageEventContent>, room: Room| {
                    let metrics = metrics.clone();
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
                        let mut metrics = metrics.lock().unwrap();
                        metrics.report_message_received(ev.event_id.to_string());
                    }
                }
            })
            .await;
        log::info!(
            "Registered event handler {} {}",
            self.id,
            instant.elapsed().as_millis()
        );

        tokio::spawn({
            let client = self.client.clone();
            async move {
                client.sync(SyncSettings::default()).await;
            }
        });

        log::info!("Spawned sync {} {}", self.id, instant.elapsed().as_millis());
        User {
            id: self.id.clone(),
            client: self.client.clone(),
            state: Synching {
                metrics: self.state.metrics.clone(),
                ..Default::default()
            },
        }
    }
}

impl User<Synching> {
    pub async fn create_room(&self) -> Option<RoomId> {
        let request = create_room::Request::new();
        let response = self.client.create_room(request).await;
        match response {
            Ok(ref response) => {
                // notify
                Some(response.room_id.clone())
            }
            Err(e) => {
                let mut metrics = self.state.metrics.lock().unwrap();
                metrics.report_error(e);
                None
            }
        }
    }

    pub async fn join_room(&mut self, room_id: &RoomId) {
        let response = self.client.join_room_by_id(room_id).await;
        match response {
            Ok(ref response) => {
                self.state.rooms.push(response.room_id.clone());
            }
            Err(e) => {
                let mut metrics = self.state.metrics.lock().unwrap();
                metrics.report_error(e);
            }
        }
    }

    async fn send_message(&self, room_id: &RoomId) {
        let content = AnyMessageEventContent::RoomMessage(MessageEventContent::text_plain(
            get_random_string(),
        ));

        let response = self.client.room_send(room_id, content, None).await;

        match response {
            Ok(response) => {
                let mut metrics = self.state.metrics.lock().unwrap();
                let event_id = response.event_id.to_string();
                metrics.report_message_sent(event_id.clone());
                log::info!(
                    "Message sent from {} to room {} with event id {}",
                    self.id,
                    room_id,
                    event_id
                )
            }
            Err(e) => {
                if let matrix_sdk::Error::Http(e) = e {
                    let mut metrics = self.state.metrics.lock().unwrap();
                    metrics.report_error(e);
                }
            }
        }
    }

    pub async fn act(&self) {
        for room_id in &self.state.rooms {
            // let counter_value = counter.load(Ordering::SeqCst) + 1;
            // counter.store(counter_value, Ordering::SeqCst);
            // log::info!("Current message counter: {}", counter_value);

            self.send_message(room_id).await;
        }
    }
}
