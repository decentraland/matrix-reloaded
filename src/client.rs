use crate::{
    configuration::{get_homeserver_url, Config},
    events::{
        Event, SyncEvent, SyncEventsSender, UserNotifications, UserNotificationsSender, UserRequest,
    },
    text::get_random_string,
    user::MessageType,
};
use async_channel::Sender;
use futures::Future;
use matrix_sdk::ruma::{
    api::{
        client::{
            account::register::v3::Request as RegistrationRequest,
            error::ErrorKind,
            membership::join_room_by_id::v3::Request as JoinRoomRequest,
            message::get_message_events::v3::Request as MessagesRequest,
            presence::set_presence::v3::Request as UpdatePresenceRequest,
            room::create_room::v3::{Request as CreateRoomRequest, RoomPreset},
            uiaa::{AuthData, Dummy, UiaaResponse},
            Error,
        },
        error::FromHttpResponseError::{self, Server},
        error::ServerError::Known,
        OutgoingRequest,
    },
    assign,
    events::{
        room::{
            create::OriginalSyncRoomCreateEvent,
            member::StrippedRoomMemberEvent,
            message::{OriginalSyncRoomMessageEvent, RoomMessageEventContent},
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
    HttpError::{self, Api, UiaaError},
    LoopCtrl, RumaApiError,
};
use std::fmt::Debug;
use std::time::{Duration, Instant};

// unbounded channel used to queue sync events like room messages or invites
type SyncChannel = (
    async_channel::Sender<SyncEvent>,
    async_channel::Receiver<SyncEvent>,
);

#[derive(Clone, Debug)]
pub struct Client {
    inner: matrix_sdk::Client,
    event_notifier: SyncEventsSender,
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
        direct_messages: Vec<OwnedRoomId>,
        invited_rooms: Vec<OwnedRoomId>,
        channels: Vec<OwnedRoomId>, // channels where user has joined
        cancel_sync: Sender<bool>,
    },
    Failed,
}

const PASSWORD: &str = "asdfasdf";

impl Client {
    pub async fn new(notifier: SyncEventsSender, config: &Config) -> Self {
        let inner = Self::create(
            &config.server.homeserver,
            config.requests.retry_enabled,
            config.server.wk_login,
        )
        .await
        .expect("Couldn't create client");
        let channel = async_channel::unbounded::<SyncEvent>();
        Self {
            inner,
            event_notifier: notifier,
            sync_channel: channel,
        }
    }

    async fn create(
        homeserver_url: &str,
        retry_enabled: bool,
        respect_login_well_known: bool,
    ) -> Result<matrix_sdk::Client, ClientBuildError> {
        let homeserver = get_homeserver_url(homeserver_url, None);

        let timeout = Duration::from_secs(30);

        let request_config = if retry_enabled {
            RequestConfig::short_retry().retry_timeout(timeout)
        } else {
            RequestConfig::new().disable_retry().timeout(timeout)
        };

        matrix_sdk::Client::builder()
            .request_config(request_config)
            .homeserver_url(homeserver)
            .respect_login_well_known(respect_login_well_known)
            .build()
            .await
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
        self.inner = client;
    }

    pub async fn login(&self, localpart: &str) -> LoginResult {
        let client = &self.inner;
        let now = Instant::now();
        let response = client.login(localpart, PASSWORD, None, None).await;
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

    pub async fn register(&self, localpart: &str) -> RegisterResult {
        let client = &self.inner;

        let req = assign!(RegistrationRequest::new(), {
            username: Some(localpart),
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

        match response {
            Err(UiaaError(Server(Known(UiaaResponse::MatrixError(Error {
                kind: ErrorKind::UserInUse,
                ..
            }))))) => RegisterResult::Ok,
            Err(e) => {
                self.event_notifier
                    .send(Event::Error((UserRequest::Register, e)))
                    .await
                    .expect("channel should not be closed");
                RegisterResult::Failed
            }
            Ok(_) => RegisterResult::Ok,
        }
    }

    pub async fn user_id(&self) -> Option<OwnedUserId> {
        self.inner.user_id().await
    }

    /// Do initial sync and return rooms and new invites. Then register event handler for future syncs and notify events.
    pub async fn sync(&self, user_notifier: &UserNotificationsSender) -> SyncResult {
        let client = &self.inner;
        let user_id = self.user_id().await.expect("user_id to be present");
        let now = Instant::now();
        let response = client.sync_once(SyncSettings::default()).await;
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::InitialSync,
                now.elapsed(),
            )))
            .await
            .expect("channel to be open");
        if response.is_err() {
            if let Some(Http(e)) = response.err() {
                self.event_notifier
                    .send(Event::Error((UserRequest::InitialSync, e)))
                    .await
                    .expect("channel open");
            }
            return SyncResult::Failed;
        }

        let (tx, _) = &self.sync_channel;

        add_invite_event_handler(client, tx, &user_id).await;
        add_room_message_event_handler(client, tx, &user_id, &self.event_notifier).await;
        add_created_room_event_handler(client, user_notifier, tx).await;

        let (cancel_sync, check_cancel) = async_channel::bounded::<bool>(1);

        tokio::spawn(sync_until_cancel(client, check_cancel).await);

        let res = response.expect("already checked it is not an error");
        let invited_rooms = res.rooms.invite.keys().cloned().collect::<Vec<_>>();

        let mut direct_messages = Vec::new();
        let mut channels = Vec::new();

        for (id, _) in res.rooms.join {
            match client.get_room(&id) {
                Some(room) => {
                    if is_channel(&room) {
                        channels.push(id);
                    } else {
                        direct_messages.push(id)
                    }
                }
                None => log::debug!("room not found in store {}", id),
            }
        }

        SyncResult::Ok {
            direct_messages,
            invited_rooms,
            cancel_sync,
            channels,
        }
    }

    /// # Panics
    ///
    /// If room_id is not one of the joined rooms or couldn't retrieve it.
    ///
    pub async fn send_message(&self, room_id: &RoomId, message: String) {
        let client = &self.inner;

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

    pub async fn add_friend(&self, friend_id: &UserId) {
        let client = &self.inner;
        // try to create room (maybe it already exists, in that case we ignore that)
        let user_id = client.user_id().await.expect("user id should be present");
        let alias = get_room_alias(&user_id, friend_id);
        let invites = [friend_id.to_owned()];
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(&alias), invite: &invites, is_direct: true });

        let now = Instant::now();
        let response = client.create_room(request).await;
        log::debug!("Create room with alias {} response: {:#?}", alias, response);
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
                log::debug!("CreateRoom failed! {}", e);
                self.event_notifier
                    .send(Event::Error((UserRequest::CreateRoom, e)))
                    .await
                    .expect("channel should not be closed");
            }
            Ok(response) => {
                log::debug!("room created and invite sent to {}!", friend_id);
                self.sync_channel
                    .0
                    .send(SyncEvent::RoomCreated(response.room_id))
                    .await
                    .expect("channel to be open");
            }
        }
    }

    pub async fn create_channel(&self, channel_name: String) {
        let client = &self.inner;
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(&channel_name), preset: Some(RoomPreset::PublicChat) });
        let now = Instant::now();

        let response = client.create_room(request).await;
        self.event_notifier
            .send(Event::RequestDuration((
                UserRequest::CreateChannel,
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
                log::debug!("CreateRoom failed! {}", e);
                self.event_notifier
                    .send(Event::Error((UserRequest::CreateRoom, e)))
                    .await
                    .expect("channel should not be closed");
            }
            Ok(response) => {
                log::debug!("channel created succesfully, {}", response.room_id);
            }
        }
    }

    pub async fn join_room(&self, room_id: &RoomId, room_type: MessageType) {
        let request = JoinRoomRequest::new(room_id);
        self.send_and_notify(request, UserRequest::JoinRoom).await;
        if let MessageType::Channel = room_type {
            self.sync_channel
                .0
                .send(SyncEvent::GetChannelMembers(room_id.to_owned()))
                .await
                .expect("channel should not be closed")
        }
    }

    pub async fn get_channel_members(&self, room_id: &RoomId) {
        let client = &self.inner;
        let now = Instant::now();

        match client.get_room(room_id) {
            Some(room) => {
                let response = room.members().await;
                match response {
                    Ok(_) => {
                        self.event_notifier
                            .send(Event::RequestDuration((
                                UserRequest::GetChannelMembers,
                                now.elapsed(),
                            )))
                            .await
                            .expect("channel should not be closed");
                    }
                    Err(e) => {
                        log::debug!("get channel members failed! {}", e);
                        if let Http(e) = e {
                            self.event_notifier
                                .send(Event::Error((UserRequest::GetChannelMembers, e)))
                                .await
                                .expect("channel should not be closed");
                        }
                    }
                }
            }
            None => {
                log::debug!("get_channel_members: room {} not found", room_id)
            }
        }
    }

    pub async fn update_status(&self) {
        let user_id = self.user_id().await.expect("user_id to be present");
        let random_status_msg = get_random_string();
        let update_presence = assign!(UpdatePresenceRequest::new(&user_id, PresenceState::Online), { status_msg: Some(random_status_msg.as_str())});
        self.send_and_notify(update_presence, UserRequest::UpdateStatus)
            .await;
    }

    pub async fn read_messages(&self, room_id: OwnedRoomId) {
        let messages_request = MessagesRequest::from_start(&room_id);
        self.send_and_notify(messages_request, UserRequest::Messages)
            .await;
    }

    async fn send_and_notify<Request>(&self, request: Request, user_request: UserRequest)
    where
        Request: OutgoingRequest + Debug,
        HttpError: From<FromHttpResponseError<Request::EndpointError>>,
    {
        let client = &self.inner;
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
    // client state is held in an `Arc` so the `Client` can be cloned freely.
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
    notifier: &SyncEventsSender,
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

async fn add_created_room_event_handler(
    client: &matrix_sdk::Client,
    user_notifier: &UserNotificationsSender,
    tx: &Sender<SyncEvent>,
) {
    client
        .register_event_handler({
            let user_notifier = user_notifier.clone();
            let tx = tx.clone();
            move |_event: OriginalSyncRoomCreateEvent, room: Room| {
                let user_notifier = user_notifier.clone();
                let tx = tx.clone();
                async move {
                    on_room_created(room, user_notifier, tx).await;
                }
            }
        })
        .await;
}

async fn on_room_created(
    room: Room,
    user_notifier: UserNotificationsSender,
    tx: Sender<SyncEvent>,
) {
    if is_channel(&room) {
        let room_id = room.room_id();
        // Notify simulation about a new channel in order to add it to the in-world state
        user_notifier
            .send(UserNotifications::NewChannel(room_id.to_owned()))
            .await
            .expect("channel to be open");
        // Notify user about the channel in order to add it to his channels list
        tx.send(SyncEvent::ChannelCreated(room_id.to_owned()))
            .await
            .expect("channel to be open");
    }
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
    notifier: &SyncEventsSender,
) {
    if let Room::Joined(room) = room {
        if let MessageType::Text(text) = event.content.msgtype {
            if event.sender.localpart() == user_id.localpart() {
                return;
            }

            log::debug!(
                "Message received! next time user {} will have someone to respond :D",
                user_id
            );

            sender
                .send(SyncEvent::MessageReceived(
                    room.room_id().to_owned(),
                    text.body,
                ))
                .await
                .expect("channel open");
            notifier
                .send(Event::MessageReceived(event.event_id.to_string()))
                .await
                .expect("channel open");
        }
    }
}

fn get_room_alias(first: &UserId, second: &UserId) -> String {
    let mut names = vec![first.localpart(), second.localpart()];
    names.sort();
    names.join("-")
}

fn is_channel(room: &Room) -> bool {
    !room.is_direct() && room.is_public()
}
