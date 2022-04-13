use crate::{time::time_now, user::UserRequest};
use matrix_sdk::HttpError;

use serde_with::serde_as;
use std::{
    cmp::Reverse,
    collections::HashMap,
    fs::{create_dir, create_dir_all, File},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct MessageTimes {
    sent: Option<Instant>,
    received: Option<Instant>,
}

#[derive(Default, Clone)]
pub struct Metrics {
    http_errors: Arc<Mutex<Vec<(HttpError, UserRequest)>>>,
    request_times: Arc<Mutex<Vec<(UserRequest, Duration)>>>,
    messages: Arc<Mutex<HashMap<String, MessageTimes>>>,
}

#[serde_as]
#[derive(serde::Serialize, Default)]
struct Report {
    requests_average_time: Vec<(UserRequest, u128)>,
    http_errors_per_request: Vec<(UserRequest, usize)>,
    message_delivery_average_time: u128,
    lost_messages: usize,
}

impl Metrics {
    pub fn report_error(&mut self, e: (HttpError, UserRequest)) {
        self.http_errors.lock().unwrap().push(e);
    }

    pub fn report_request_duration(&mut self, request: (UserRequest, Duration)) {
        self.request_times.lock().unwrap().push(request);
    }

    pub fn report_message_sent(&mut self, message_id: String) {
        self.messages
            .lock()
            .unwrap()
            .entry(message_id)
            .or_default()
            .sent = Some(Instant::now());
    }

    pub fn report_message_received(&mut self, message_id: String) {
        self.messages
            .lock()
            .unwrap()
            .entry(message_id)
            .or_default()
            .received = Some(Instant::now());
    }

    fn calculate_requests_average_time(&self) -> Vec<(UserRequest, u128)> {
        let request_times = self.request_times.lock().unwrap();
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

    fn calculate_message_delivery_average_time(&self) -> u128 {
        let messages = self.messages.lock().unwrap();
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

    fn calculate_lost_messages(&self) -> usize {
        let messages = self.messages.lock().unwrap();
        messages
            .iter()
            .filter(|(_, times)| times.received.is_none())
            .count()
    }

    pub fn all_messages_received(&self) -> bool {
        self.calculate_lost_messages() == 0
    }

    fn calculate_http_errors_per_request(&self) -> Vec<(UserRequest, usize)> {
        Vec::from_iter(self.http_errors.lock().unwrap().iter().fold(
            HashMap::<UserRequest, usize>::new(),
            |mut map, (_, request_type)| {
                *map.entry(request_type.clone()).or_default() += 1;
                map
            },
        ))
    }

    pub fn generate_report(&self, output_dir: String) {
        let mut http_errors_per_request = self.calculate_http_errors_per_request();
        let mut requests_average_time = self.calculate_requests_average_time();
        let message_delivery_average_time = self.calculate_message_delivery_average_time();

        requests_average_time.sort_unstable_by_key(|(_, time)| Reverse(*time));
        http_errors_per_request.sort_unstable_by_key(|(_, count)| Reverse(*count));

        let lost_messages = self.calculate_lost_messages();

        let report = Report {
            requests_average_time,
            message_delivery_average_time,
            http_errors_per_request,
            lost_messages,
        };

        let result = create_dir_all(&output_dir);
        let output_dir = if result.is_err() {
            println!("Couldn't ensure output folder, defaulting to 'output/'");
            create_dir("output").unwrap();
            "output".to_string()
        } else {
            output_dir
        };

        let path = format!("{}/report_{}.yaml", output_dir, time_now());
        let buffer = File::create(&path).unwrap();

        serde_yaml::to_writer(buffer, &report).expect("Couldn't write report to file");
        println!("Report generated: {}", path);
    }
}
