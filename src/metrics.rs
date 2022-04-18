use crate::events::Event;
use crate::{time::time_now, user::UserRequest};
use futures::lock::Mutex;
use matrix_sdk::HttpError;
use serde_with::serde_as;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::{
    cmp::Reverse,
    collections::HashMap,
    fs::{create_dir_all, File},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Receiver;

#[derive(Default)]
struct MessageTimes {
    sent: Option<Instant>,
    received: Option<Instant>,
}

pub struct Metrics {
    all_messages_received: Arc<AtomicBool>,
    receiver: Arc<Mutex<Receiver<Event>>>,
}

#[serde_as]
#[derive(serde::Serialize, Default)]
pub struct Report {
    requests_average_time: Vec<(UserRequest, u128)>,
    http_errors_per_request: Vec<(String, usize)>,
    message_delivery_average_time: u128,
    lost_messages: usize,
}

impl Report {
    fn from(
        http_errors: &[(UserRequest, HttpError)],
        request_times: &[(UserRequest, Duration)],
        messages: &HashMap<String, MessageTimes>,
    ) -> Self {
        let mut http_errors_per_request = calculate_http_errors_per_request(http_errors);
        let mut requests_average_time = calculate_requests_average_time(request_times);
        let message_delivery_average_time = calculate_message_delivery_average_time(messages);

        requests_average_time.sort_unstable_by_key(|(_, time)| Reverse(*time));
        http_errors_per_request.sort_unstable_by_key(|(_, count)| Reverse(*count));

        let lost_messages = calculate_lost_messages(messages);

        Self {
            requests_average_time,
            http_errors_per_request,
            message_delivery_average_time,
            lost_messages,
        }
    }

    pub fn generate_report(&self, execution_id: u128, step: usize, output_dir: &str) {
        let result = create_dir_all(format!("{}/{}", output_dir, execution_id));
        let output_dir = if result.is_err() {
            println!(
                "Couldn't ensure output folder, defaulting to 'output/{}'",
                execution_id
            );
            create_dir_all(format!("output/{}", execution_id)).unwrap();
            "output"
        } else {
            output_dir
        };

        let path = format!(
            "{}/{}/report_{}_{}.yaml",
            output_dir,
            execution_id,
            step,
            time_now()
        );
        let buffer = File::create(&path).unwrap();

        serde_yaml::to_writer(buffer, &self).expect("Couldn't write report to file");
        println!("Report generated: {}", path);
    }
}

impl Metrics {
    pub fn new(receiver: Receiver<Event>) -> Self {
        Self {
            all_messages_received: Arc::new(AtomicBool::default()),
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    pub async fn all_messages_received(&self) -> bool {
        self.all_messages_received
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn run(&self) -> tokio::task::JoinHandle<Report> {
        tokio::spawn(read_events(
            self.receiver.clone(),
            self.all_messages_received.clone(),
        ))
    }
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

fn calculate_message_delivery_average_time(messages: &HashMap<String, MessageTimes>) -> u128 {
    let total = messages.iter().fold(0, |total, (_, times)| {
        if let Some(sent) = times.sent {
            if let Some(received) = times.received {
                return total + (received.duration_since(sent)).as_millis();
            }
        }
        total
    });

    total / (messages.len() as u128)
}

fn calculate_lost_messages(messages: &HashMap<String, MessageTimes>) -> usize {
    messages
        .iter()
        .filter(|(_, times)| times.received.is_none())
        .count()
}

async fn read_events(
    receiver: Arc<Mutex<Receiver<Event>>>,
    all_messages_received: Arc<AtomicBool>,
) -> Report {
    let mut http_errors: Vec<(UserRequest, HttpError)> = vec![];
    let mut request_times: Vec<(UserRequest, Duration)> = vec![];
    let mut messages: HashMap<String, MessageTimes> = HashMap::new();

    let mut finishing_phase = false;

    let mut rec = receiver.lock().await;

    loop {
        match rec.recv().await {
            Some(e) => {
                log::info!("Event received {:?}", e);
                match e {
                    Event::Error(e) => {
                        http_errors.push(e);
                    }
                    Event::MessageSent(message_id) => {
                        messages.entry(message_id).or_default().sent = Some(Instant::now());
                    }
                    Event::MessageReceived(message_id) => {
                        messages.entry(message_id).or_default().received = Some(Instant::now());
                        if finishing_phase {
                            check_and_swap_all_messages_received(&messages, &all_messages_received);
                        }
                    }
                    Event::RequestDuration(request) => {
                        request_times.push(request);
                    }
                    Event::AllMessagesSent => {
                        finishing_phase = true;
                    }
                    Event::Finish => break Report::from(&http_errors, &request_times, &messages),
                }
            }
            None => {
                log::info!("Failed to receive an event through the mpsc chanel.");
            }
        }
    }
}

fn check_and_swap_all_messages_received(
    messages: &HashMap<String, MessageTimes>,
    all_messages_received: &AtomicBool,
) {
    if calculate_lost_messages(messages) == 0 {
        all_messages_received.swap(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn get_error_code(e: &HttpError) -> String {
    use matrix_sdk::ruma::api::error::*;
    match e {
        HttpError::ClientApi(FromHttpResponseError::Http(ServerError::Known(e))) => {
            e.status_code.to_string()
        }
        HttpError::Server(status_code) => status_code.to_string(),
        HttpError::UiaaError(FromHttpResponseError::Http(ServerError::Known(e))) => match e {
            matrix_sdk::ruma::api::client::r0::uiaa::UiaaResponse::AuthResponse(e) => {
                e.auth_error.as_ref().unwrap().message.clone()
            }
            matrix_sdk::ruma::api::client::r0::uiaa::UiaaResponse::MatrixError(e) => {
                e.kind.to_string()
            }
            _ => e.to_string(),
        },
        _ => e.to_string(),
    }
}

fn calculate_http_errors_per_request(
    http_errors: &[(UserRequest, HttpError)],
) -> Vec<(String, usize)> {
    Vec::from_iter(http_errors.iter().fold(
        HashMap::<String, usize>::new(),
        |mut map, (request_type, e)| {
            let error_code = get_error_code(e);
            *map.entry(format!("{:?}:{:?}", request_type.clone(), error_code))
                .or_default() += 1;
            map
        },
    ))
}
