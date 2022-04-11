use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::instant::Instant;
use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::membership::join_room_by_id;
use matrix_sdk::ruma::api::client::r0::message::send_message_event;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::session::login;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageEventContent, SyncMessageEvent};
use matrix_sdk::{Client, ClientConfig, HttpResult, RequestConfig, SyncSettings};

use matrix_sdk::ruma::{assign, RoomId, UserId};

use crate::text::get_random_string;

const PASSWORD: &str = "asdfasdf";

pub struct User {
    id: UserId,
    client: Client,
}

impl User {
    pub async fn new(id: String, homeserver: String) -> Result<Self, String> {
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
        .await
        .expect("Couldn't create new client");

        log::info!("new client {} {}", user_id, instant.elapsed().as_millis());

        Ok(Self {
            id: user_id,
            client,
        })
    }

    pub fn id(&self) -> UserId {
        self.id.clone()
    }

    pub async fn register(&self) -> HttpResult<register::Response> {
        let instant = Instant::now();

        let req = assign!(register::Request::new(), {
            username: Some(self.id.localpart()),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });
        let response = self.client.register(req).await;

        log::info!(
            "registered req {} {}",
            self.id,
            instant.elapsed().as_millis()
        );
        response
    }

    pub async fn login(&self) -> Result<login::Response, matrix_sdk::Error> {
        let instant = Instant::now();

        let response = self
            .client
            .login(self.id.localpart(), PASSWORD, None, None)
            .await;
        log::info!(
            "login new client {} {}",
            self.id,
            instant.elapsed().as_millis()
        );
        response
    }

    pub async fn sync(&self, counter: &Arc<AtomicUsize>) {
        let instant = Instant::now();

        self.client
            .register_event_handler({
                let counter = counter.clone();
                let user_id = self.id.clone();
                move |ev: SyncMessageEvent<MessageEventContent>, room: Room| {
                    let counter = counter.clone();
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
                        let actual_value = counter.load(Ordering::SeqCst);
                        log::info!("Messages counter before decrease: {}", actual_value);

                        counter.store(actual_value - 1, Ordering::SeqCst);
                        log::info!("Messages counter after decrease: {}", actual_value - 1);
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
    }

    pub async fn create_room(&self) -> Result<create_room::Response, matrix_sdk::HttpError> {
        let request = create_room::Request::new();
        let response = self.client.create_room(request).await;
        match response {
            Ok(ref response) => {
                // notify
            }
            Err(ref e) => {
                // report
            }
        }
        response
    }

    pub async fn join_room(
        &self,
        room_id: &RoomId,
    ) -> Result<join_room_by_id::Response, matrix_sdk::HttpError> {
        let response = self.client.join_room_by_id(&room_id).await;
        match response {
            Ok(ref response) => {
                // notify join
            }
            Err(ref e) => {
                // report error
            }
        }
        response
    }

    pub async fn send_message(
        &self,
        room_id: &RoomId,
    ) -> Result<send_message_event::Response, matrix_sdk::Error> {
        let content = AnyMessageEventContent::RoomMessage(MessageEventContent::text_plain(
            get_random_string(),
        ));

        let response = self.client.room_send(room_id, content, None).await;

        if let Ok(ref sent_message) = response {
            let event_id = sent_message.event_id.to_string();
            log::info!(
                "Message sent from {} to room {} with event id {}",
                self.id,
                room_id,
                event_id
            );
        }

        response
    }
}
