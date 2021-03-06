use crate::{
    configuration::{get_homeserver_url, Config},
    events::{Event, Notifier, SyncEvent, UserRequest},
    text::get_random_string,
};
use async_channel::Sender;
use futures::{lock::Mutex, Future};
use matrix_sdk::ruma::{
    api::{
        client::{
            account::register::v3::Request as RegistrationRequest,
            error::ErrorKind,
            membership::join_room_by_id::v3::Request as JoinRoomRequest,
            presence::set_presence::v3::Request as UpdatePresenceRequest,
            room::create_room::v3::Request as CreateRoomRequest,
            uiaa::{AuthData, Dummy},
            Error,
        },
        error::FromHttpResponseError::{self, Server},
        error::ServerError::Known,
        OutgoingRequest,
    },
    assign,
    events::{
        room::{
            member::StrippedRoomMemberEvent,
            message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
        },
        AnyMessageLikeEventContent,
    },
    presence::PresenceState,
    OwnedRoomId, OwnedUserId, RoomId, UserId,
};
use matrix_sdk::{
    config::{RequestConfig, SyncSettings},
    room::Room,
    ClientBuildError,
    Error::Http,
    HttpError::{self, Api},
    LoopCtrl, RumaApiError,
};
use std::fmt::Debug;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

// unbounded channel used to queue sync events like room messages or invites
type SyncChannel = (
    async_channel::Sender<SyncEvent>,
    async_channel::Receiver<SyncEvent>,
);

#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Mutex<matrix_sdk::Client>>,
    event_notifier: Notifier,
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
    pub async fn new(notifier: Notifier, config: &Config) -> Self {
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
            event_notifier: notifier,
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

    pub async fn reset(&mut self, config: &Config) {
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
        self.event_notifier
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
                    self.event_notifier
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
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::Register,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        if let Err(e) = response {
            self.event_notifier
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
        let now = Instant::now();
        let response = client.sync_once(SyncSettings::default()).await;
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::InitialSync,
                now.elapsed(),
            )))
            .await
            .expect("channel to be open");
        let (tx, _) = &self.sync_channel;

        add_invite_event_handler(&client, tx, user_id).await;
        add_room_message_event_handler(&client, tx, user_id, &self.event_notifier).await;

        let (cancel_sync, check_cancel) = async_channel::bounded::<bool>(1);

        tokio::spawn(sync_until_cancel(&client, check_cancel).await);

        match response {
            Ok(res) => {
                let joined_rooms = res.rooms.join.keys().cloned().collect::<Vec<_>>();
                let invited_rooms = res.rooms.invite.keys().cloned().collect::<Vec<_>>();
                SyncResult::Ok {
                    joined_rooms,
                    invited_rooms,
                    cancel_sync,
                }
            }
            Err(Http(e)) => {
                self.event_notifier
                    .send(Event::Error((UserRequest::Login, e)))
                    .await
                    .expect("channel open");
                SyncResult::Failed
            }
            _ => SyncResult::Failed,
        }
    }

    /// # Panics
    ///
    /// If room_id is not one of the joined rooms or couldn't retrieve it.
    ///
    pub async fn send_message(&self, room_id: &RoomId, message: String) {
        let client = self.inner.lock().await;

        let content =
            AnyMessageLikeEventContent::RoomMessage(RoomMessageEventContent::text_plain(message));

        let room = client
            .get_joined_room(room_id)
            .unwrap_or_else(|| panic!("cannot get joined room {}", room_id));

        let now = Instant::now();
        let response = room.send(content, None).await;
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::SendMessage,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        match response {
            Ok(response) => {
                self.event_notifier
                    .send(Event::MessageSent(response.event_id.to_string()))
                    .await
                    .expect("channel open");
            }
            Err(Http(e)) => {
                self.event_notifier
                    .send(Event::Error((UserRequest::Login, e)))
                    .await
                    .expect("channel open");
            }
            _ => {}
        }
    }

    pub async fn add_friend(&self, user_id: &UserId) {
        // try to create room (maybe it already exists, in that case we ignore that)
        let alias = user_id.localpart();
        let invites = [user_id.to_owned()];
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(alias), invite: &invites, is_direct: true });

        let now = Instant::now();
        let client = self.inner.lock().await;
        let response = client.create_room(request).await;
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::CreateRoom,
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");

        match response {
            Err(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::RoomInUse,
                ..
            }))))) => log::debug!("CreateRoom failed but it was already created"),
            Err(e) => {
                self.event_notifier
                    .send(Event::Error((UserRequest::CreateRoom, e)))
                    .await
                    .expect("channel should not be closed");
            }
            Ok(_) => {}
        }
    }

    pub async fn join_room(&self, room_id: &RoomId) {
        let request = JoinRoomRequest::new(room_id);
        self.send_and_notify(request, UserRequest::JoinRoom).await;
    }

    pub async fn update_status(&self, user_id: &UserId) {
        let random_status_msg = get_random_string();
        let update_presence = assign!(UpdatePresenceRequest::new(user_id, PresenceState::Online), { status_msg: Some(random_status_msg.as_str())});
        self.send_and_notify(update_presence, UserRequest::UpdateStatus)
            .await;
    }

    async fn send_and_notify<Request>(&self, request: Request, user_request: UserRequest)
    where
        Request: OutgoingRequest + Debug,
        HttpError: From<FromHttpResponseError<Request::EndpointError>>,
    {
        let client = self.inner.lock().await;
        let now = Instant::now();
        let response = client.send(request, None).await;
        self.event_notifier
            .send(Event::RequestDuration((
                user_request.clone(),
                now.elapsed(),
            )))
            .await
            .expect("channel should not be closed");
        if let Err(e) = response {
            self.event_notifier
                .send(Event::Error((user_request, e)))
                .await
                .expect("channel should not be closed");
        }
    }
}

async fn sync_until_cancel(
    client: &matrix_sdk::Client,
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
    client: &matrix_sdk::Client,
    tx: &Sender<SyncEvent>,
    user_id: &UserId,
    notifier: &Notifier,
) {
    client
        .register_event_handler({
            let tx = tx.clone();
            let user_id = user_id.to_owned();
            let notifier = notifier.clone();
            move |event, room| {
                let tx = tx.clone();
                let user_id = user_id.clone();
                let notifier = notifier.clone();
                async move {
                    on_room_message(event, room, tx, user_id, &notifier).await;
                }
            }
        })
        .await;
}

async fn add_invite_event_handler(
    client: &matrix_sdk::Client,
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
    // ignore invitation when it doesn't affect the current user
    if room_member.state_key != user_id {
        return;
    }
    if let Room::Invited(room) = room {
        log::debug!("user {} was invited to room {}!", user_id, room.room_id());
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
    notifier: &Notifier,
) {
    if let Room::Joined(room) = room {
        if let MessageType::Text(text) = event.content.msgtype {
            if event.sender.localpart() == user_id.localpart() {
                return;
            }
            room.read_receipt(&event.event_id)
                .await
                .expect("can send read receipt");

            log::debug!(
                "Message received! next time user {} will have someone to respond :D",
                user_id
            );

            sender
                .send(SyncEvent::Message(room.room_id().to_owned(), text.body))
                .await
                .expect("channel open");
            notifier
                .send(Event::MessageReceived(event.event_id.to_string()))
                .await
                .expect("channel open");
        }
    }
}
