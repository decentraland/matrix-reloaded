use std::time::Instant;

use matrix_sdk::HttpError;

#[derive(Default)]
pub struct Metrics {
    pub http_errors: usize,
    pub sent_messages: Vec<(String, Instant)>,
    pub received_messages: Vec<(String, Instant)>,
}

impl Metrics {
    pub fn report_error(&mut self, e: HttpError) {
        log::info!("Http error reported: {}", e);
        self.http_errors += 1;
    }

    pub fn report_message_sent(&mut self, message_id: String) {
        self.sent_messages.push((message_id, Instant::now()));
    }

    pub fn report_message_received(&mut self, message_id: String) {
        self.received_messages.push((message_id, Instant::now()));
    }
}
