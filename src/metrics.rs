use crate::events::Event;
use crate::{time::time_now, user::UserRequest};
use futures::lock::Mutex;
use matrix_sdk::HttpError;
use serde_with::serde_as;

use std::{
    cmp::Reverse,
    collections::HashMap,
    fs::{create_dir_all, File},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{self, Receiver, Sender};

#[derive(Default)]
struct MessageTimes {
    sent: Option<Instant>,
    received: Option<Instant>,
}

pub struct Metrics {
    http_errors: Arc<Mutex<Vec<(UserRequest, HttpError)>>>,
    request_times: Arc<Mutex<Vec<(UserRequest, Duration)>>>,
    messages: Arc<Mutex<HashMap<String, MessageTimes>>>,
}

#[serde_as]
#[derive(serde::Serialize, Default)]
struct Report {
    requests_average_time: Vec<(UserRequest, u128)>,
    http_errors_per_request: Vec<(String, usize)>,
    message_delivery_average_time: u128,
    lost_messages: usize,
}

impl Metrics {
    pub fn new() -> (Self, Sender<Event>) {
        let http_errors: Arc<Mutex<Vec<(UserRequest, HttpError)>>> = Arc::new(Mutex::new(vec![]));
        let request_times: Arc<Mutex<Vec<(UserRequest, Duration)>>> = Arc::new(Mutex::new(vec![]));
        let messages: Arc<Mutex<HashMap<String, MessageTimes>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (tx, rx) = mpsc::channel::<Event>(100);
        let receiver = Arc::new(Mutex::new(rx));

        tokio::spawn(read_events(
            receiver,
            http_errors.clone(),
            request_times.clone(),
            messages.clone(),
        ));
        (
            Self {
                http_errors,
                request_times,
                messages,
            },
            tx,
        )
    }

    async fn calculate_requests_average_time(&self) -> Vec<(UserRequest, u128)> {
        let request_times = self.request_times.lock().await;
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

    async fn calculate_message_delivery_average_time(&self) -> u128 {
        let messages = self.messages.lock().await;
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

    async fn calculate_lost_messages(&self) -> usize {
        self.messages
            .lock()
            .await
            .iter()
            .filter(|(_, times)| times.received.is_none())
            .count()
    }

    pub async fn all_messages_received(&self) -> bool {
        self.calculate_lost_messages().await == 0
    }

    async fn calculate_http_errors_per_request(&self) -> Vec<(String, usize)> {
        Vec::from_iter(self.http_errors.lock().await.iter().fold(
            HashMap::<String, usize>::new(),
            |mut map, (request_type, e)| {
                let error_code = get_error_code(e);
                *map.entry(format!("{:?}:{:?}", request_type.clone(), error_code))
                    .or_default() += 1;
                map
            },
        ))
    }

    pub async fn generate_report(&self, execution_id: u128, step: usize, output_dir: &str) {
        let mut http_errors_per_request = self.calculate_http_errors_per_request().await;
        let mut requests_average_time = self.calculate_requests_average_time().await;
        let message_delivery_average_time = self.calculate_message_delivery_average_time().await;

        requests_average_time.sort_unstable_by_key(|(_, time)| Reverse(*time));
        http_errors_per_request.sort_unstable_by_key(|(_, count)| Reverse(*count));

        let lost_messages = self.calculate_lost_messages().await;

        let report = Report {
            requests_average_time,
            message_delivery_average_time,
            http_errors_per_request,
            lost_messages,
        };

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

        serde_yaml::to_writer(buffer, &report).expect("Couldn't write report to file");
        println!("Report generated: {}", path);
    }
}

async fn read_events(
    receiver: Arc<Mutex<Receiver<Event>>>,
    http_errors: Arc<Mutex<Vec<(UserRequest, HttpError)>>>,
    request_times: Arc<Mutex<Vec<(UserRequest, Duration)>>>,
    messages: Arc<Mutex<HashMap<String, MessageTimes>>>,
) {
    let mut receiver = receiver.lock().await;
    loop {
        match receiver.recv().await {
            Some(e) => {
                log::info!("Event received {:?}", e);
                match e {
                    Event::Error(e) => http_errors.lock().await.push(e),
                    Event::MessageSent(message_id) => {
                        messages.lock().await.entry(message_id).or_default().sent =
                            Some(Instant::now())
                    }
                    Event::MessageReceived(message_id) => {
                        messages
                            .lock()
                            .await
                            .entry(message_id)
                            .or_default()
                            .received = Some(Instant::now())
                    }
                    Event::RequestDuration(request) => request_times.lock().await.push(request),
                }
            }
            None => {
                log::info!("Failed to receive an event through the mpsc chanel.");
            }
        }
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
