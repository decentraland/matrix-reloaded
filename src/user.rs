use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::error::ErrorKind;
use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy, UiaaResponse};
use matrix_sdk::ruma::api::error::{FromHttpResponseError::*, ServerError::*};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageEventContent, SyncMessageEvent};
use matrix_sdk::HttpError::UiaaError;
use matrix_sdk::{Client, ClientConfig, RequestConfig, SyncSettings};
use rand::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;

use matrix_sdk::ruma::{assign, RoomId, UserId};

use crate::events::Event;
use crate::text::get_random_string;

const PASSWORD: &str = "asdfasdf";

pub struct Disconnected;
pub struct Registered;
pub struct LoggedIn;
#[derive(Clone)]
pub struct Synching {
    rooms: Arc<Mutex<Vec<RoomId>>>,
}

#[derive(Clone)]
pub struct User<State> {
    id: UserId,
    client: Arc<tokio::sync::Mutex<Client>>,
    tx: Sender<Event>,
    state: State,
}

impl<State> User<State> {
    pub fn id(&self) -> UserId {
        self.id.clone()
    }

    pub async fn send(&self, event: Event) {
        log::info!("Sending event {:?}", event);
        if self.tx.send(event).await.is_err() {
            log::info!("Receiver dropped");
        }
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
        tx: Sender<Event>,
    ) -> Option<User<Disconnected>> {
        // init client connection (register + login)
        let user_id = UserId::try_from(format!("@{id}:{homeserver}")).unwrap();

        let instant = Instant::now();

        log::info!("Attempt to create a client with id {}", user_id);
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
            log::info!("Failed to create client");
            return None;
        }

        log::info!(
            "New client created {} {}",
            user_id,
            instant.elapsed().as_millis()
        );

        Some(Self {
            id: user_id,
            client: Arc::new(tokio::sync::Mutex::new(client.unwrap())),
            tx,
            state: Disconnected {},
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
                self.send(Event::RequestDuration((
                    UserRequest::Register,
                    instant.elapsed(),
                )))
                .await;
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    tx: self.tx.clone(),
                    state: Registered {},
                })
            }
            Err(e) => {
                // if ID is already taken, proceed as Registered
                if let UiaaError(Http(Known(UiaaResponse::MatrixError(e)))) = e {
                    if e.kind == ErrorKind::UserInUse {
                        log::info!("Client already registered, proceed to Login {}", self.id());
                        let user = User::new(
                            self.id.localpart(),
                            self.id.server_name().as_str(),
                            false,
                            self.tx.clone(),
                        )
                        .await
                        .unwrap();
                        return Some(User {
                            id: user.id,
                            client: user.client,
                            tx: user.tx,
                            state: Registered {},
                        });
                    }
                } else {
                    self.send(Event::Error((UserRequest::Register, e))).await;
                }
                None
            }
        }
    }
}

impl User<Registered> {
    pub async fn login(&mut self) -> Option<User<LoggedIn>> {
        let instant = Instant::now();

        let client = self.client.lock().await;
        log::info!("Attempt to login client with id {}", self.id());
        let response = client
            .login(self.id.localpart(), PASSWORD, None, None)
            .await;

        log::info!("Login response: {:?}", response);
        match response {
            Ok(_) => {
                self.send(Event::RequestDuration((
                    UserRequest::Login,
                    instant.elapsed(),
                )))
                .await;
                Some(User {
                    id: self.id.clone(),
                    client: self.client.clone(),
                    tx: self.tx.clone(),
                    state: LoggedIn {},
                })
            }
            Err(e) => {
                if let matrix_sdk::Error::Http(e) = e {
                    self.send(Event::Error((UserRequest::Login, e))).await;
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
                let tx = self.tx.clone();
                let user_id = self.id.clone();
                move |ev: SyncMessageEvent<MessageEventContent>, room: Room| {
                    let tx = tx.clone();
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
                        tx.send(Event::MessageReceived(ev.event_id.to_string()))
                            .await
                            .expect("Receiver dropped");
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
            tx: self.tx.clone(),
            state: Synching {
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
                self.send(Event::RequestDuration((
                    UserRequest::CreateRoom,
                    instant.elapsed(),
                )))
                .await;
                Some(response.room_id.clone())
            }
            Err(e) => {
                self.send(Event::Error((UserRequest::CreateRoom, e))).await;
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
                self.send(Event::RequestDuration((
                    UserRequest::JoinRoom,
                    instant.elapsed(),
                )))
                .await;
                self.state.rooms.lock().await.push(response.room_id.clone());
            }
            Err(e) => {
                self.send(Event::Error((UserRequest::JoinRoom, e))).await;
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
                self.send(Event::RequestDuration((
                    UserRequest::SendMessage,
                    instant.elapsed(),
                )))
                .await;

                self.send(Event::MessageSent(response.event_id.to_string()))
                    .await;
            }
            Err(e) => {
                if let matrix_sdk::Error::Http(e) = e {
                    self.send(Event::Error((UserRequest::SendMessage, e))).await;
                }
            }
        }
    }
}
