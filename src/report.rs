use crate::events::MessageTimes;
use crate::events::UserRequest;
use matrix_sdk::ruma::api::client::uiaa::UiaaResponse;
use matrix_sdk::ruma::api::error::*;
use matrix_sdk::HttpError;
use matrix_sdk::RumaApiError;
use serde::Serialize;
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use std::fs::create_dir_all;
use std::fs::File;
use std::{cmp::Reverse, collections::HashMap, time::Duration};

#[serde_as]
#[derive(Serialize, Default, Debug)]
pub struct Report {
    #[serde_as(as = "HashMap<_, _>")]
    requests_average_time: Vec<(UserRequest, u128)>,
    #[serde_as(as = "HashMap<_, _>")]
    total_requests: Vec<(UserRequest, u128)>,
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    http_errors_per_request: Vec<(String, usize)>,
    message_delivery_average_time: Option<u128>,
    /// number of messages sent correctly but not received (receipent is offline)
    messages_sent: usize,
    /// number of messages received that do not match with sent
    messages_not_sent: usize,
    /// number of messages sent and received during simulation
    real_time_messages: usize,
}

impl Report {
    pub fn from(
        http_errors: &[(UserRequest, HttpError)],
        request_times: &[(UserRequest, Duration)],
        messages: &HashMap<String, MessageTimes>,
    ) -> Self {
        let mut http_errors_per_request = Self::calculate_http_errors_per_request(http_errors);
        let mut requests_average_time = Self::calculate_requests_average_time(request_times);
        let total_requests_by_request = Self::total_requests_by_request(request_times);

        let message_delivery_average_time = Self::calculate_message_delivery_average_time(messages);

        requests_average_time.sort_unstable_by_key(|(_, time)| Reverse(*time));
        http_errors_per_request.sort_unstable_by_key(|(_, count)| Reverse(*count));

        let (real_time_messages, messages_sent, messages_not_sent, unknown_messages) =
            Self::classify_messages(messages);

        log::debug!(
            "there were {} unknown messages (sent nor received)",
            unknown_messages
        );

        Self {
            requests_average_time,
            total_requests: total_requests_by_request,
            http_errors_per_request,
            message_delivery_average_time,
            messages_not_sent,
            messages_sent,
            real_time_messages,
        }
    }

    fn get_error_code(e: &HttpError) -> String {
        match e {
            HttpError::Api(FromHttpResponseError::Server(ServerError::Known(
                RumaApiError::ClientApi(e),
            ))) => e.status_code.as_u16().to_string(),
            HttpError::Server(status_code) => status_code.as_u16().to_string(),
            HttpError::UiaaError(FromHttpResponseError::Server(ServerError::Known(e))) => match e {
                UiaaResponse::AuthResponse(e) => e.auth_error.as_ref().unwrap().message.clone(),
                UiaaResponse::MatrixError(e) => e.kind.to_string(),
                _ => e.to_string(),
            },
            HttpError::Reqwest(e) => {
                if e.is_request() {
                    log::error!("{}", e);
                    "failed_to_send_request".to_string()
                } else {
                    e.to_string()
                }
            }
            _ => e.to_string(),
        }
    }

    fn total_requests_by_request(
        request_times: &[(UserRequest, Duration)],
    ) -> Vec<(UserRequest, u128)> {
        request_times
            .iter()
            .fold(
                HashMap::<UserRequest, u128>::new(),
                |mut map, (request, _)| {
                    *map.entry(request.clone()).or_default() += 1;

                    map
                },
            )
            .iter()
            .map(|(req, count)| (req.clone(), *count))
            .collect()
    }

    fn calculate_requests_average_time(
        request_times: &[(UserRequest, Duration)],
    ) -> Vec<(UserRequest, u128)> {
        request_times
            .iter()
            .fold(
                HashMap::<UserRequest, Vec<u128>>::new(),
                |mut map, (request, duration)| {
                    map.entry(request.clone())
                        .or_default()
                        .push(duration.as_millis());
                    map
                },
            )
            .iter()
            .map(|(request, times)| {
                (
                    request.clone(),
                    times.iter().sum::<u128>() / (times.len() as u128),
                )
            })
            .collect()
    }

    fn calculate_message_delivery_average_time(
        messages: &HashMap<String, MessageTimes>,
    ) -> Option<u128> {
        if messages.is_empty() {
            return None;
        }

        let messages_sent_and_received = messages
            .iter()
            .filter(|(_, times)| times.sent.is_some() && times.received.is_some());

        let total = messages_sent_and_received.fold(0, |total, (_, times)| {
            let MessageTimes { sent, received } = times;
            match (sent, received) {
                (Some(sent), Some(received)) => {
                    total + (received.duration_since(*sent)).as_millis()
                }
                _ => total,
            }
        });

        if total == 0 {
            None
        } else {
            Some(total / (messages.len() as u128))
        }
    }

    fn calculate_http_errors_per_request(
        http_errors: &[(UserRequest, HttpError)],
    ) -> Vec<(String, usize)> {
        Vec::from_iter(http_errors.iter().fold(
            HashMap::<String, usize>::new(),
            |mut map, (request_type, e)| {
                let error_code = Self::get_error_code(e);
                *map.entry(format!("{}_{}", request_type.clone(), error_code))
                    .or_default() += 1;
                map
            },
        ))
    }

    fn classify_messages(messages: &HashMap<String, MessageTimes>) -> (usize, usize, usize, usize) {
        let mut messages_sent = 0;
        let mut messages_not_sent = 0;
        let mut unknown_messages = 0;
        let mut real_time_messages = 0;

        for times in messages.values() {
            let received = times.received.is_some();
            let sent = times.sent.is_some();

            match (sent, received) {
                (true, true) => real_time_messages += 1,
                (true, false) => messages_sent += 1,
                (false, true) => messages_not_sent += 1,
                (false, false) => unknown_messages += 1,
            };
        }

        (
            real_time_messages,
            messages_sent,
            messages_not_sent,
            unknown_messages,
        )
    }

    pub fn generate(&self, output_dir: &str, execution_id: &str) {
        let reports_dir = Self::ensure_execution_directory(output_dir, execution_id);

        let path = format!("{reports_dir}/report_{execution_id}.yaml");
        let buffer = File::create(&path).unwrap();

        serde_yaml::to_writer(buffer, self).expect("couldn't write report to file");

        println!("Final report generated: {}\n", path);
        println!("{:#?}\n", self);
    }

    fn compute_reports_dir(output_dir: &str, execution_id: &str) -> String {
        format!("{}/{}", output_dir, execution_id)
    }

    ///
    /// Ensures the existence of the output and execution directories and the capacity of the tool
    /// to create files and write to both.
    ///
    /// # Panics
    ///
    /// If we are not able to create the directory for the current execution.
    ///
    fn ensure_execution_directory(output_dir: &str, execution_id: &str) -> String {
        let directory = Self::compute_reports_dir(output_dir, execution_id);

        create_dir_all(directory.clone())
            .unwrap_or_else(|_| panic!("could not create output directory {}", directory));
        directory
    }
}
