use crate::{
    configuration::{get_homeserver_url, Config},
    events::{
        Event, SyncEvent, SyncEventsSender, UserNotifications, UserNotificationsSender, UserRequest,
    },
    room::RoomType,
    text::get_random_string,
};
use async_channel::Sender;
use futures::Future;
use matrix_sdk::ruma::{
    api::{
        client::{
            account::register::v3::Request as RegistrationRequest,
            error::ErrorKind,
            membership::join_room_by_id::v3::Request as JoinRoomRequest,
            membership::leave_room::v3::Request as LeaveRoomRequest,
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
            join_rules::OriginalSyncRoomJoinRulesEvent,
            member::StrippedRoomMemberEvent,
            message::{
                MessageType as MatrixMessageType, OriginalSyncRoomMessageEvent,
                RoomMessageEventContent,
            },
        },
        AnyMessageLikeEventContent,
    },
    presence::PresenceState,
    OwnedRoomId, OwnedUserId, RoomId, RoomOrAliasId, ServerName, UserId,
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
        rooms: Vec<(OwnedRoomId, RoomType)>,
        invited_rooms: Vec<OwnedRoomId>,
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

        while let Ok(event) = rv.try_recv() {
            events.push(event);
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
        let login_builder = self.inner.login_username(localpart, PASSWORD);

        let response = self
            .instrument(UserRequest::Login, || async { login_builder.send().await })
            .await;

        match response {
            Ok(_) => LoginResult::Ok,
            Err(Http(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::NotFound | ErrorKind::Forbidden,
                ..
            })))))) => LoginResult::NotRegistered,
            Err(e) => {
                if let Http(e) = e {
                    self.notify_error(UserRequest::Login, e).await;
                }
                LoginResult::Failed
            }
        }
    }

    pub async fn register(&self, localpart: &str) -> RegisterResult {
        let req = assign!(RegistrationRequest::new(), {
            username: Some(localpart),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });

        let response = self
            .instrument(UserRequest::Register, || async {
                self.inner.register(req).await
            })
            .await;

        match response {
            Err(UiaaError(Server(Known(UiaaResponse::MatrixError(Error {
                kind: ErrorKind::UserInUse,
                ..
            }))))) => RegisterResult::Ok,
            Err(e) => {
                self.notify_error(UserRequest::Register, e).await;
                RegisterResult::Failed
            }
            Ok(_) => RegisterResult::Ok,
        }
    }

    pub fn user_id(&self) -> Option<&UserId> {
        self.inner.user_id()
    }

    /// Do initial sync and return rooms and new invites. Then register event handler for future syncs and notify events.
    pub async fn sync(
        &self,
        user_notifier: &UserNotificationsSender,
        presence_enabled: bool,
    ) -> SyncResult {
        let client = &self.inner;
        let user_id = self.user_id().expect("user_id to be present");
        let user_presence = if presence_enabled {
            PresenceState::Online
        } else {
            PresenceState::Offline
        };
        let response = self
            .instrument(UserRequest::InitialSync, || async {
                client
                    .sync_once(SyncSettings::default().set_presence(user_presence))
                    .await
            })
            .await;
        match response {
            Err(_) => {
                if let Some(Http(e)) = response.err() {
                    self.notify_error(UserRequest::InitialSync, e).await;
                }
                SyncResult::Failed
            }
            Ok(_) => {
                let (tx, _) = &self.sync_channel;

                add_invite_event_handler(client, tx, user_id).await;
                add_room_message_event_handler(client, tx, user_id, &self.event_notifier).await;
                add_room_join_rules_event_handler(client, user_notifier, tx).await;

                let (cancel_sync, check_cancel) = async_channel::bounded::<bool>(1);

                tokio::spawn(sync_until_cancel(client, check_cancel).await);

                let res = response.expect("already checked it is not an error");
                let invited_rooms = res.rooms.invite.keys().cloned().collect::<Vec<_>>();

                let mut rooms = Vec::new();

                for (id, _) in res.rooms.join {
                    match client.get_room(&id) {
                        Some(room) => {
                            if is_channel(&room) {
                                rooms.push((id, RoomType::Channel))
                            } else {
                                rooms.push((id, RoomType::DirectMessage))
                            }
                        }
                        None => log::debug!("room not found in store {}", id),
                    }
                }

                SyncResult::Ok {
                    rooms,
                    invited_rooms,
                    cancel_sync,
                }
            }
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

        let response = self
            .instrument(UserRequest::SendMessage, || async {
                room.send(content, None).await
            })
            .await;

        match response {
            Ok(response) => {
                let event = Event::MessageSent(response.event_id.to_string());
                self.notify_event(event).await;
            }
            Err(Http(e)) => {
                self.notify_error(UserRequest::SendMessage, e).await;
            }
            _ => {}
        }
    }

    pub async fn add_friend(&self, friend_id: &UserId) {
        let client = &self.inner;
        // try to create room (maybe it already exists, in that case we ignore that)
        let user_id = client.user_id().expect("user id should be present");
        let alias = get_room_alias(user_id, friend_id);
        let invites = [friend_id.to_owned()];
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(&alias), invite: &invites, is_direct: true, preset: Some(RoomPreset::TrustedPrivateChat) });
        let response = self
            .instrument(UserRequest::CreateRoom, || async {
                client.create_room(request).await
            })
            .await;
        log::debug!("Create room with alias {} response: {:#?}", alias, response);

        match response {
            Err(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::RoomInUse,
                ..
            }))))) => log::debug!("CreateRoom failed but it was already created"),
            Err(e) => {
                log::debug!("CreateRoom failed! {}", e);
                self.notify_error(UserRequest::CreateRoom, e).await;
            }
            Ok(response) => {
                log::debug!("room created and invite sent to {}!", friend_id);
                self.notify_sync(SyncEvent::RoomCreated(response.room_id))
                    .await;
            }
        }
    }

    pub async fn create_channel(&self, channel_name: String) {
        let request = assign!(CreateRoomRequest::new(), { room_alias_name: Some(&channel_name), preset: Some(RoomPreset::PublicChat) });
        let response = self
            .instrument(UserRequest::CreateChannel, || async {
                self.inner.create_room(request).await
            })
            .await;

        match response {
            Err(Api(Server(Known(RumaApiError::ClientApi(Error {
                kind: ErrorKind::RoomInUse,
                ..
            }))))) => log::debug!("CreateChannel failed but it was already created"),
            Err(e) => {
                log::debug!("CreateChannel failed! {}", e);
                self.notify_error(UserRequest::CreateChannel, e).await;
            }
            Ok(response) => {
                log::debug!("channel created succesfully, {}", response.room_id);
            }
        }
    }

    pub async fn join_room(
        &self,
        room_id: &RoomId,
        room_type: RoomType,
        allow_get_channel_members: bool,
    ) {
        let request = JoinRoomRequest::new(room_id);
        self.send_and_notify(request, UserRequest::JoinRoom).await;
        if allow_get_channel_members {
            if let RoomType::Channel = room_type {
                self.notify_sync(SyncEvent::GetChannelMembers(room_id.to_owned()))
                    .await;
            }
        }
    }

    pub async fn join_channel(&self, channel_name: String) {
        let client = &self.inner;
        let user_homeserver = self.user_id().unwrap().server_name();
        let channel_alias = format!("#{channel_name}:{user_homeserver}");
        let room_alias = <&RoomOrAliasId>::try_from(channel_alias.as_str()).unwrap();

        log::debug!("join_channel by alias {}", room_alias);
        let result = client
            .join_room_by_id_or_alias(room_alias, &[user_homeserver.into()])
            .await;

        if let Err(err) = result {
            log::debug!("Failed to join channel! {:?}", err);
        }
    }

    pub async fn get_channel_members(&self, room_id: &RoomId) {
        self.instrument(UserRequest::GetChannelMembers, || async {
            match self.inner.get_room(room_id) {
                None => log::debug!("get_channel_members: room {} not found", room_id),
                Some(room) => {
                    if let Err(Http(e)) = room.members().await {
                        log::debug!("get channel members failed! {}", e);
                        self.notify_error(UserRequest::GetChannelMembers, e).await;
                    }
                }
            }
        })
        .await;
    }

    pub async fn leave_room(&self, room_id: OwnedRoomId) {
        let req = LeaveRoomRequest::new(&room_id);
        self.send_and_notify(req, UserRequest::LeaveChannel).await;
    }

    pub async fn update_status(&self) {
        let user_id = self.user_id().expect("user_id to be present");
        let random_status_msg = get_random_string();
        let update_presence = assign!(UpdatePresenceRequest::new(user_id, PresenceState::Online), { status_msg: Some(random_status_msg.as_str())});
        self.send_and_notify(update_presence, UserRequest::UpdateStatus)
            .await;
    }

    pub async fn read_messages(&self, room_id: OwnedRoomId) {
        let messages_request = MessagesRequest::forward(&room_id);
        self.send_and_notify(messages_request, UserRequest::Messages)
            .await;
    }

    async fn send_and_notify<Request>(&self, request: Request, user_request: UserRequest)
    where
        Request: OutgoingRequest + Debug,
        HttpError: From<FromHttpResponseError<Request::EndpointError>>,
    {
        let response = self
            .instrument(user_request.clone(), || async {
                self.inner.send(request, None).await
            })
            .await;

        if let Err(e) = response {
            self.notify_error(user_request, e).await;
        }
    }
    async fn instrument<F, Fut, Result>(&self, user_request: UserRequest, send_request: F) -> Result
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result>,
    {
        let now = Instant::now();
        let result = send_request().await;
        self.notify_event(Event::RequestDuration((
            user_request.clone(),
            now.elapsed(),
        )))
        .await;
        result
    }

    async fn notify_event(&self, event: Event) {
        self.event_notifier
            .send(event)
            .await
            .expect("channel should not be closed");
    }

    async fn notify_error(&self, user_request: UserRequest, error: HttpError) {
        self.notify_event(Event::Error((user_request, error))).await
    }

    async fn notify_sync(&self, msg: SyncEvent) {
        self.sync_channel
            .0
            .send(msg)
            .await
            .expect("channel should not be closed")
    }
}

async fn sync_until_cancel(
    client: &matrix_sdk::Client,
    check_cancel: async_channel::Receiver<bool>,
) -> impl Future<Output = ()> {
    // client state is held in an `Arc` so the `Client` can be cloned freely.
    let client = client.clone();
    async move {
        match client
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
            .await
        {
            Ok(_) => {}
            Err(err) => log::debug!("error while syncing an user - {:?}", err),
        }
    }
}

async fn add_room_message_event_handler(
    client: &matrix_sdk::Client,
    tx: &Sender<SyncEvent>,
    user_id: &UserId,
    notifier: &SyncEventsSender,
) {
    client.add_event_handler({
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
    });
}

async fn add_invite_event_handler(
    client: &matrix_sdk::Client,
    tx: &Sender<SyncEvent>,
    user_id: &UserId,
) {
    client.add_event_handler({
        let tx = tx.clone();
        let user_id = user_id.to_owned();
        move |event, room| {
            let tx = tx.clone();
            let user_id = user_id.clone();
            async move {
                on_room_member_event(event, room, tx, user_id).await;
            }
        }
    });
}

async fn add_room_join_rules_event_handler(
    client: &matrix_sdk::Client,
    user_notifier: &UserNotificationsSender,
    tx: &Sender<SyncEvent>,
) {
    client.add_event_handler({
        let user_notifier = user_notifier.clone();
        let tx = tx.clone();
        move |_event: OriginalSyncRoomJoinRulesEvent, room: Room| {
            let user_notifier = user_notifier.clone();
            let tx = tx.clone();
            async move {
                on_room_join_rules(room, user_notifier, tx).await;
            }
        }
    });
}

async fn on_room_join_rules(
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

async fn on_room_member_event(
    room_member: StrippedRoomMemberEvent,
    room: Room,
    sender: Sender<SyncEvent>,
    user_id: OwnedUserId,
) {
    // ignore event when it doesn't affect the current user
    if room_member.state_key != user_id {
        return;
    }
    if let Room::Invited(room) = &room {
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
    if let Room::Joined(joined_room) = &room {
        if let MatrixMessageType::Text(text) = event.content.msgtype {
            if event.sender.localpart() == user_id.localpart() {
                return;
            }

            let message_type = if is_channel(&room) {
                RoomType::Channel
            } else {
                RoomType::DirectMessage
            };

            log::debug!(
                "Message {:?} received! next time user {} will have someone to respond :D",
                message_type,
                user_id
            );

            sender
                .send(SyncEvent::MessageReceived(
                    joined_room.room_id().to_owned(),
                    text.body,
                    message_type,
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
    room.is_public()
}
