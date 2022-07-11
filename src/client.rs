use crate::{
    configuration::{get_homeserver_url, SimulationConfig},
    events::{Event, Notifier, SyncEvent, UserRequest},
};
use async_channel::Sender;
use futures::{lock::Mutex, Future};
use matrix_sdk::ruma::{
    api::{
        client::{
            account::register::v3::Request as RegistrationRequest,
            error::ErrorKind,
            room::create_room::v3::Request as CreateRoomRequest,
            uiaa::{AuthData, Dummy},
            Error,
        },
        error::FromHttpResponseError::Server,
        error::ServerError::Known,
    },
    assign,
    events::{
        room::{
            member::StrippedRoomMemberEvent,
            message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
        },
        AnyMessageLikeEventContent,
    },
    OwnedRoomId, OwnedUserId, RoomId, UserId,
};
use matrix_sdk::{
    config::{RequestConfig, SyncSettings},
    room::Room,
    ClientBuildError,
    Error::Http,
    HttpError::Api,
    LoopCtrl, RumaApiError,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::sleep;

// unbounded channel used to queue sync events like room messages or invites
type SyncChannel = (
    async_channel::Sender<SyncEvent>,
    async_channel::Receiver<SyncEvent>,
);

#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Mutex<matrix_sdk::Client>>,
    metrics_notifier: Notifier,
    sync_channel: SyncChannel,
}

pub enum LoginResult {
    Ok,
    NotRegistered,
    Failed,
}

pub enum RegisterResult {
    Ok,
    Failed,
}

pub enum SyncResult {
    Ok {
        joined_rooms: Vec<OwnedRoomId>,
        invited_rooms: Vec<OwnedRoomId>,
        cancel_sync: Sender<bool>,
    },
    Failed,
}

const PASSWORD: &str = "asdfasdf";

impl Client {
    pub async fn new(notifier: Notifier, config: &SimulationConfig) -> Self {
        let inner = Self::create(
            &config.server.homeserver,
            config.requests.retry_enabled,
            config.server.wk_login,
        )
        .await
        .expect("Couldn't create client");
        let channel = async_channel::unbounded::<SyncEvent>();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            metrics_notifier: notifier,
            sync_channel: channel,
        }
    }

    async fn create(
        homeserver_url: &str,
        retry_enabled: bool,
        respect_login_well_known: bool,
    ) -> Result<matrix_sdk::Client, ClientBuildError> {
        let (_, homeserver) = get_homeserver_url(homeserver_url, None);

        let timeout = Duration::from_secs(30);

        let request_config = if retry_enabled {
            RequestConfig::new().retry_timeout(timeout)
        } else {
            RequestConfig::new().disable_retry().timeout(timeout)
        };

        let client = matrix_sdk::Client::builder()
            .request_config(request_config)
            .homeserver_url(homeserver)
            .respect_login_well_known(respect_login_well_known)
            .build()
            .await;

        client
    }

    pub async fn read_sync_events(&self) -> Vec<SyncEvent> {
        let (_, rv) = &self.sync_channel;
        let mut events = vec![];

        while !rv.is_empty() {
            if let Ok(event) = rv.recv().await {
                events.push(event);
            }
        }
        events
    }

    pub async fn reset(&mut self, config: &SimulationConfig) {
        let client = Self::create(
            &config.server.homeserver,
            config.requests.retry_enabled,
            config.server.wk_login,
        )
        .await
        .expect("Couldn't create client");
        self.inner = Arc::new(Mutex::new(client));
    }

    pub async fn login(&self, id: &UserId) -> LoginResult {
        let client = self.inner.lock().await;
        let now = Instant::now();
        let response = client.login(id.localpart(), PASSWORD, None, None).await;
        self.metrics_notifier
            .send(Event::RequestDuration((UserRequest::Login, now.elapsed())))
            .await
            .expect("channel should not be closed");
        match response {
            Ok(_) => LoginResult::Ok,
            Err(Http(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::NotFound,
                ..
            })))))) => LoginResult::NotRegistered,
            Err(Http(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::Forbidden,
                ..
            })))))) => LoginResult::NotRegistered,
            Err(e) => {
                if let Http(e) = e {
                    self.metrics_notifier
                        .send(Event::Error((UserRequest::Login, e)))
                        .await
                        .expect("channel should not be closed");
                }
                LoginResult::Failed
            }
        }
    }

    pub async fn register(&self, id: &UserId) -> RegisterResult {
        let client = self.inner.lock().await;

        let req = assign!(RegistrationRequest::new(), {
            username: Some(id.localpart()),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });
        let now = Instant::now();
        let response = client.register(req).await;
        self.metrics_notifier
            .send(Event::RequestDuration((
                UserRequest::Register,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        if let Err(e) = response {
            self.metrics_notifier
                .send(Event::Error((UserRequest::Login, e)))
                .await
                .expect("channel should not be closed");
            RegisterResult::Failed
        } else {
            RegisterResult::Ok
        }
    }

    /// Do initial sync and return rooms and new invites. Then register event handler for future syncs and notify events.
    pub async fn sync(&self, user_id: &UserId) -> SyncResult {
        let client = self.inner.lock().await;
        let response = client.sync_once(SyncSettings::default()).await;
        let (tx, _) = &self.sync_channel;

        add_room_message_event_handler(&client, tx, user_id).await;
        add_auto_join_event_handler(&client, tx, user_id).await;

        let (cancel_sync, check_cancel) = async_channel::bounded::<bool>(1);

        tokio::spawn(sync_until_cancel(client, check_cancel).await);

        if let Ok(res) = response {
            let joined_rooms = res.rooms.join.keys().cloned().collect::<Vec<_>>();
            let invited_rooms = res.rooms.invite.keys().cloned().collect::<Vec<_>>();
            SyncResult::Ok {
                joined_rooms,
                invited_rooms,
                cancel_sync,
            }
        } else {
            SyncResult::Failed
        }
    }

    pub async fn send_message(&self, room_id: &RoomId, message: String) {
        let client = self.inner.lock().await;

        let content =
            AnyMessageLikeEventContent::RoomMessage(RoomMessageEventContent::text_plain(message));

        let room = client
            .get_joined_room(room_id)
            .unwrap_or_else(|| panic!("cannot get joined room {}", room_id));

        let now = Instant::now();
        let response = room.send(content, None).await;
        self.metrics_notifier
            .send(Event::RequestDuration((
                UserRequest::SendMessage,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        if let Err(Http(e)) = response {
            self.metrics_notifier
                .send(Event::Error((UserRequest::Login, e)))
                .await
                .expect("channel should not be closed");
        }
    }

    pub async fn add_friend(&self, user_id: &UserId) {
        // try to create room (maybe it already exists, in that case we ignore that)
        let client = self.inner.lock().await;
        let alias = user_id.localpart();
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(alias) });

        let now = Instant::now();
        let response = client.create_room(request).await;
        self.metrics_notifier
            .send(Event::RequestDuration((
                UserRequest::CreateRoom,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        if let Err(e) = response {
            self.metrics_notifier
                .send(Event::Error((UserRequest::CreateRoom, e)))
                .await
                .expect("channel should not be closed");
        }
    }

    pub async fn join_room(&self, room_id: &RoomId) {
        let client = self.inner.lock().await;

        let now = Instant::now();
        let response = client.join_room_by_id(room_id).await;
        self.metrics_notifier
            .send(Event::RequestDuration((
                UserRequest::JoinRoom,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");
        if let Err(e) = response {
            self.metrics_notifier
                .send(Event::Error((UserRequest::JoinRoom, e)))
                .await
                .expect("channel should not be closed");
        }
    }
}

async fn sync_until_cancel(
    client: futures::lock::MutexGuard<'_, matrix_sdk::Client>,
    check_cancel: async_channel::Receiver<bool>,
) -> impl Future<Output = ()> {
    // we are not cloning the mutex to avoid locking it forever
    let client = client.clone();
    async move {
        client
            .sync_with_callback(SyncSettings::default(), {
                let check_cancel = check_cancel.clone();
                move |_| {
                    let check_cancel = check_cancel.clone();
                    async move {
                        if check_cancel.try_recv().is_ok() {
                            LoopCtrl::Break
                        } else {
                            LoopCtrl::Continue
                        }
                    }
                }
            })
            .await;
    }
}

async fn add_room_message_event_handler(
    client: &futures::lock::MutexGuard<'_, matrix_sdk::Client>,
    tx: &Sender<SyncEvent>,
    user_id: &UserId,
) {
    client
        .register_event_handler({
            let tx = tx.clone();
            let user_id = user_id.to_owned();
            move |event, room| {
                let tx = tx.clone();
                let user_id = user_id.clone();
                async move {
                    on_room_message(event, room, tx, user_id).await;
                }
            }
        })
        .await;
}

async fn add_auto_join_event_handler(
    client: &futures::lock::MutexGuard<'_, matrix_sdk::Client>,
    tx: &Sender<SyncEvent>,
    user_id: &UserId,
) {
    client
        .register_event_handler({
            let tx = tx.clone();
            let user_id = user_id.to_owned();
            move |event, room| {
                let tx = tx.clone();
                let user_id = user_id.clone();
                async move {
                    on_room_invite(event, room, tx, user_id).await;
                }
            }
        })
        .await;
}

async fn on_room_invite(
    room_member: StrippedRoomMemberEvent,
    room: Room,
    sender: Sender<SyncEvent>,
    user_id: OwnedUserId,
) {
    if room_member.state_key != user_id {
        return;
    }

    if let Room::Invited(room) = room {
        let mut delay = 2;

        while let Err(err) = room.accept_invitation().await {
            // retry autojoin due to synapse sending invites, before the
            // invited user can join for more information see
            // https://github.com/matrix-org/synapse/issues/4345
            sleep(Duration::from_secs(delay)).await;
            delay *= 2;

            if delay > 3600 {
                log::error!(
                    "user {} couldn't join room {} ({:?})",
                    user_id,
                    room.room_id(),
                    err
                );
                break;
            }
        }
        log::info!("user {} autojoined room {}", user_id, room.room_id());
        sender
            .send(SyncEvent::Invite(room.room_id().to_owned()))
            .await
            .expect("channel to be open");
    }
}

async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    sender: Sender<SyncEvent>,
    user_id: OwnedUserId,
) {
    if let Room::Joined(room) = room {
        if let MessageType::Text(text) = event.content.msgtype {
            if event.sender.localpart() == user_id.localpart() {
                return;
            }
            log::info!(
                "Message received! next time user {} will have someone to respond :D",
                user_id
            );
            sender
                .send(SyncEvent::Message(room.room_id().to_owned(), text.body))
                .await
                .expect("channel to be open");
        }
    }
}
