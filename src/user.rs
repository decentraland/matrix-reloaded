use crate::events::Event;
use crate::text::get_random_string;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::error::ErrorKind;
use matrix_sdk::ruma::api::client::uiaa::{AuthData, Dummy, UiaaResponse};
use matrix_sdk::ruma::api::error::FromHttpResponseError::Server;
use matrix_sdk::ruma::api::error::ServerError::Known;
use matrix_sdk::ruma::{
    api::client::{
        account::register::v3::Request as RegistrationRequest,
        room::create_room::v3::Request as CreateRoomRequest,
    },
    assign,
};
use matrix_sdk::ruma::{RoomId, UserId};
use matrix_sdk::Client;
use matrix_sdk::HttpError::UiaaError;
use matrix_sdk::{
    config::SyncSettings,
    ruma::events::{
        room::message::{OriginalSyncRoomMessageEvent, RoomMessageEventContent},
        AnyMessageLikeEventContent,
    },
};
use rand::Rng;
use regex::Regex;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;

const PASSWORD: &str = "asdfasdf";

pub struct Disconnected;
pub struct Registered;
pub struct LoggedIn;
#[derive(Clone)]
pub struct Synching {
    rooms: Arc<Mutex<Vec<Box<RoomId>>>>,
}

#[derive(Clone)]
pub struct User<State> {
    id: Box<UserId>,
    client: Arc<tokio::sync::Mutex<Client>>,
    tx: Sender<Event>,
    state: State,
}

impl<State> User<State> {
    pub fn id(&self) -> &UserId {
        self.id.as_ref()
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
        // TODO: check which protocol we want to use: http or https (defaulting to https)
        let (homeserver_no_protocol, homeserver_url) = get_homeserver_url(homeserver, None);

        let user_id = UserId::parse(format!("@{id}:{homeserver_no_protocol}").as_str()).unwrap();

        let instant = Instant::now();

        log::info!("Attempt to create a client with id {}", user_id);

        let request_config = if retry_enabled {
            RequestConfig::new().retry_timeout(Duration::from_secs(30))
        } else {
            RequestConfig::new()
                .disable_retry()
                .timeout(Duration::from_secs(30))
        };

        let client = Client::builder()
            .request_config(request_config)
            .homeserver_url(homeserver_url)
            .build()
            .await;
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

        let req = assign!(RegistrationRequest::new(), {
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
                if let UiaaError(Server(Known(UiaaResponse::MatrixError(e)))) = e {
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
                move |ev, room| {
                    let tx = tx.clone();
                    let user_id = user_id.clone();
                    async move {
                        on_room_message(ev, room, tx, user_id).await;
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
    pub async fn create_room(&mut self) -> Option<Box<RoomId>> {
        let client = self.client.lock().await;

        let instant = Instant::now();
        let request = CreateRoomRequest::new();
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
        let content = AnyMessageLikeEventContent::RoomMessage(RoomMessageEventContent::text_plain(
            get_random_string(),
        ));
        let instant = Instant::now();

        let room = client
            .get_joined_room(room_id)
            .expect("Room must be Joined at this point");
        let response = room.send(content, None).await;
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

async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    sender: Sender<Event>,
    user_id: Box<UserId>,
) {
    if let Room::Joined(room) = room {
        if event.sender.localpart() == user_id.localpart() {
            return;
        }
        sender
            .send(Event::MessageReceived(event.event_id.to_string()))
            .await
            .expect("Receiver dropped");
        log::info!(
            "User {} received a message from room {} and sent by {}",
            user_id,
            room.room_id(),
            event.sender
        );
    }
}

/// This function returns homeserver domain and url, ex:
///  - get_homeserver_url("matrix.domain.com") => ("matrix.domain.com", "https://matrix.domain.com")
fn get_homeserver_url<'a>(homeserver: &'a str, protocol: Option<&'a str>) -> (&'a str, String) {
    let regex = Regex::new(r"https?://").unwrap();
    if regex.is_match(homeserver) {
        let parts: Vec<&str> = regex.splitn(homeserver, 2).collect();
        (parts[1], homeserver.to_string())
    } else {
        let protocol = protocol.unwrap_or("https");
        (homeserver, format!("{protocol}://{homeserver}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::user::*;
    #[test]
    fn homeserver_arg_can_start_with_https() {
        let homeserver_arg = "https://matrix.domain.com";
        assert_eq!(
            ("matrix.domain.com", homeserver_arg.to_string()),
            get_homeserver_url(homeserver_arg, None)
        );
    }

    #[test]
    fn homeserver_arg_can_start_with_http() {
        let homeserver_arg = "http://matrix.domain.com";

        assert_eq!(
            ("matrix.domain.com", homeserver_arg.to_string()),
            get_homeserver_url(homeserver_arg, None)
        );
    }

    #[test]
    fn homeserver_arg_can_start_without_protocol() {
        let homeserver_arg = "matrix.domain.com";
        let expected_homeserver_url = "https://matrix.domain.com";

        assert_eq!(
            (homeserver_arg, expected_homeserver_url.to_string()),
            get_homeserver_url(homeserver_arg, None)
        );
    }

    #[test]
    fn homeserver_should_return_specified_protocol() {
        let homeserver_arg = "matrix.domain.com";
        let expected_homeserver_url = "http://matrix.domain.com";

        assert_eq!(
            (homeserver_arg, expected_homeserver_url.to_string()),
            get_homeserver_url(homeserver_arg, Some("http"))
        );
    }
}
