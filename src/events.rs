use crate::user::UserRequest;
use matrix_sdk::HttpError;
use std::time::Duration;

#[derive(Debug)]
pub enum Event {
    Error((UserRequest, HttpError)),
    MessageSent(String),
    MessageReceived(String),
    RequestDuration((UserRequest, Duration)),
}
