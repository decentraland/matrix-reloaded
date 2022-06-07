use std::collections::BTreeMap;

use async_trait::async_trait;
use matrix_sdk::reqwest::StatusCode;
use matrix_sdk::ruma::api::client::config::get_global_account_data::v3::{
    Request as GetAccountDataRequest, Response as GetAccountDataResponse,
};
use matrix_sdk::ruma::api::client::config::set_global_account_data::v3::{
    Request as SetAccountDataRequest, Response as SetAccountDataResponse,
};
use matrix_sdk::ruma::api::client::presence::set_presence::v3::Request as SetPresenceRequest;
use matrix_sdk::ruma::api::client::presence::set_presence::v3::Response as SetPresenceResponse;
use matrix_sdk::ruma::api::error::FromHttpResponseError::Server;
use matrix_sdk::ruma::api::error::ServerError::Known;
use matrix_sdk::ruma::events::direct::DirectEventContent;
use matrix_sdk::ruma::events::GlobalAccountDataEventType;
use matrix_sdk::ruma::presence::PresenceState;
use matrix_sdk::ruma::{self, OwnedRoomId, OwnedUserId};
use matrix_sdk::HttpError::Api;
use matrix_sdk::{Client, RumaApiError};
use matrix_sdk::{HttpError, HttpResult};

#[async_trait]
pub trait ClientExt {
    async fn send_presence(&self, online: bool) -> HttpResult<SetPresenceResponse>;
    async fn get_account_data(&self) -> HttpResult<GetAccountDataResponse>;
    async fn set_account_data(
        &self,
        data: &DirectEventContent,
    ) -> HttpResult<SetAccountDataResponse>;
    async fn add_room_to_list(
        &self,
        friend_user_id: OwnedUserId,
        room_id: OwnedRoomId,
    ) -> Result<(), HttpError>;
}

#[async_trait]
impl ClientExt for Client {
    async fn send_presence(&self, online: bool) -> HttpResult<SetPresenceResponse> {
        let user_id = self.user_id().await.ok_or(HttpError::UserIdRequired)?;
        let presence = if online {
            PresenceState::Online
        } else {
            PresenceState::Offline
        };
        let request = SetPresenceRequest::new(&user_id, presence);
        self.send(request, None).await
    }

    async fn get_account_data(&self) -> HttpResult<GetAccountDataResponse> {
        let user_id = self.user_id().await.ok_or(HttpError::UserIdRequired)?;
        let request = GetAccountDataRequest::new(&user_id, "m.direct");
        self.send(request, None).await
    }
    async fn set_account_data(
        &self,
        data: &DirectEventContent,
    ) -> HttpResult<SetAccountDataResponse> {
        let user_id = self.user_id().await.ok_or(HttpError::UserIdRequired)?;
        let request = SetAccountDataRequest::new(data, &user_id)
            .ok()
            .ok_or(HttpError::UnableToCloneRequest)?;
        let response = self.send(request, None).await;
        response
    }

    async fn add_room_to_list(
        &self,
        friend_user_id: OwnedUserId,
        room_id: OwnedRoomId,
    ) -> Result<(), HttpError> {
        let response = self.get_account_data().await;
        let content = match response {
            Ok(response) => {
                let account_data = response.account_data;
                let mut content = account_data
                    .cast::<DirectEventContent>()
                    .deserialize_content(GlobalAccountDataEventType::Direct)
                    .expect("Received account_data should be deserializable to DirectEventContent");
                content.entry(friend_user_id).or_default().push(room_id);
                Ok(content)
            }
            Err(Api(Server(Known(RumaApiError::ClientApi(ruma::api::client::Error {
                kind: _,
                message: _,
                status_code: StatusCode::NOT_FOUND,
            }))))) => Ok(DirectEventContent(BTreeMap::new())),
            Err(http_error) => Err(http_error),
        };
        match content {
            Ok(content) => {
                if let Err(e) = self.set_account_data(&content).await {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }
}
