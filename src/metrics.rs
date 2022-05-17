use crate::events::Event;
use crate::user::UserRequest;
use futures::lock::Mutex;
use matrix_sdk::ruma::api::client::uiaa::UiaaResponse;
use matrix_sdk::HttpError;
use serde::Serialize;
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{
    cmp::Reverse,
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

type MessagesClassification = (
    HashMap<String, MessageTimes>,
    HashMap<String, MessageTimes>,
    HashMap<String, MessageTimes>,
    HashMap<String, MessageTimes>,
);

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
#[derive(Serialize, Default, Debug)]
pub struct MetricsReport {
    #[serde_as(as = "HashMap<_, _>")]
    requests_average_time: Vec<(UserRequest, u128)>,
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    http_errors_per_request: Vec<(String, usize)>,
    message_delivery_average_time: Option<u128>,
    /// number of messages sent correctly
    messages_sent: usize,
    /// number of messages received that do not match with sent
    messages_not_sent: usize,
    /// number of messages sent but not received
    lost_messages: usize,
}

impl MetricsReport {
    fn from(
        http_errors: &[(UserRequest, HttpError)],
        request_times: &[(UserRequest, Duration)],
        messages: HashMap<String, MessageTimes>,
    ) -> Self {
        let mut http_errors_per_request = calculate_http_errors_per_request(http_errors);
        let mut requests_average_time = calculate_requests_average_time(request_times);

        let messages_sent = messages
            .iter()
            .filter(|(_, times)| times.sent.is_some())
            .count();

        let (messages_not_received, messages_not_sent, messages, _) = classify_messages(messages);
        let message_delivery_average_time = calculate_message_delivery_average_time(&messages);

        requests_average_time.sort_unstable_by_key(|(_, time)| Reverse(*time));
        http_errors_per_request.sort_unstable_by_key(|(_, count)| Reverse(*count));

        let lost_messages = messages_not_received.len();
        let messages_not_sent = messages_not_sent.len();

        Self {
            requests_average_time,
            http_errors_per_request,
            message_delivery_average_time,
            messages_not_sent,
            messages_sent,
            lost_messages,
        }
    }

    pub fn any_error(&self) -> bool {
        self.http_errors_per_request.iter().any(|&(_, n)| n > 0)
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
        self.all_messages_received.load(Ordering::SeqCst)
    }

    pub fn run(&self) -> JoinHandle<MetricsReport> {
        tokio::spawn(read_events(
            self.receiver.clone(),
            self.all_messages_received.clone(),
        ))
    }
}

///
/// # Panics
/// If message sent event is processed and the message_id is already present in the messages map
/// If message received event is processed  and the message_id is not present in the messages map
///
async fn read_events(
    receiver: Arc<Mutex<Receiver<Event>>>,
    all_messages_received: Arc<AtomicBool>,
) -> MetricsReport {
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
                    Event::Finish => {
                        break MetricsReport::from(&http_errors, &request_times, messages)
                    }
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
        all_messages_received.swap(true, Ordering::SeqCst);
    }
}

fn get_error_code(e: &HttpError) -> String {
    use matrix_sdk::ruma::api::error::*;
    match e {
        HttpError::ClientApi(FromHttpResponseError::Server(ServerError::Known(e))) => {
            e.status_code.as_u16().to_string()
        }
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

    let total = messages.iter().fold(0, |total, (_, times)| {
        if let Some(sent) = times.sent {
            if let Some(received) = times.received {
                return total + (received.duration_since(sent)).as_millis();
            }
        }
        total
    });

    Some(total / (messages.len() as u128))
}

fn calculate_lost_messages(messages: &HashMap<String, MessageTimes>) -> usize {
    messages
        .iter()
        .filter(|(_, times)| times.received.is_none())
        .count()
}

fn calculate_http_errors_per_request(
    http_errors: &[(UserRequest, HttpError)],
) -> Vec<(String, usize)> {
    Vec::from_iter(http_errors.iter().fold(
        HashMap::<String, usize>::new(),
        |mut map, (request_type, e)| {
            let error_code = get_error_code(e);
            *map.entry(format!("{}_{}", request_type.clone(), error_code))
                .or_default() += 1;
            map
        },
    ))
}

fn classify_messages(messages: HashMap<String, MessageTimes>) -> MessagesClassification {
    let mut messages_not_received = HashMap::<String, MessageTimes>::new();
    let mut messages_not_sent = HashMap::<String, MessageTimes>::new();
    let mut messages_sent_and_received = HashMap::<String, MessageTimes>::new();
    let mut other_messages = HashMap::<String, MessageTimes>::new();

    for (id, times) in messages {
        let received = times.received.is_some();
        let sent = times.sent.is_some();

        match (sent, received) {
            (true, false) => messages_not_received.insert(id, times),
            (false, true) => messages_not_sent.insert(id, times),
            (true, true) => messages_sent_and_received.insert(id, times),
            (false, false) => other_messages.insert(id, times),
        };
    }

    (
        messages_not_received,
        messages_not_sent,
        messages_sent_and_received,
        other_messages,
    )
}
