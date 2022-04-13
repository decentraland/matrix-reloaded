use friendship::{Friendship, FriendshipID};
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use metrics::Metrics;
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::thread::sleep;
use std::time::{Duration, Instant};
use text::create_progress_bar;
use time::time_now;
use tokio::time::interval;
use user::{Synching, User};

mod friendship;
mod metrics;
mod text;
mod time;
mod user;

#[derive(Serialize, Deserialize, Debug)]
pub struct Configuration {
    pub homeserver_url: String,
    pub output_dir: String,
    pub total_steps: usize,
    pub users_per_step: usize,
    pub friendship_ratio: f32,
    pub step_duration_in_secs: usize,
    pub max_users_to_act_per_tick: usize,
    pub waiting_period: usize,
}

pub struct State {
    config: Configuration,
    friendships: Vec<Friendship>,
    users: Vec<User<Synching>>,
    metrics: Metrics,
}

impl Display for State {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(w, "Amount of users: {}", self.users.len()).unwrap();
        writeln!(w, "Amount of friendships: {}", self.friendships.len()).unwrap();
        writeln!(w, "Listing friendships: ").unwrap();
        for friendship in &self.friendships {
            writeln!(w, "- {}", friendship).unwrap();
        }

        Ok(())
    }
}

impl State {
    pub fn new(config: Configuration) -> Self {
        Self {
            config,
            friendships: vec![],
            users: vec![],
            metrics: Metrics::default(),
        }
    }

    async fn init_users(&mut self) {
        let timestamp = time_now();
        let actual_users = self.users.len();
        let desired_users = actual_users + self.config.users_per_step;
        let server = self.config.homeserver_url.clone();

        let mut handles = vec![];

        let progress_bar = create_progress_bar(
            "Init users".to_string(),
            (desired_users - actual_users).try_into().unwrap(),
        );
        progress_bar.tick();

        for i in actual_users..desired_users {
            handles.push(tokio::spawn({
                let server = server.clone();
                let metrics = self.metrics.clone();
                let progress_bar = progress_bar.clone();
                async move {
                    let id = format!("user_{i}_{timestamp}");
                    let user = User::new(id, server, metrics).await;

                    if let Some(mut user) = user {
                        if let Some(mut user) = user.register().await {
                            if let Some(user) = user.login().await {
                                log::info!("User is now synching: {}", user.id());
                                progress_bar.inc(1);
                                return Some(user.sync().await);
                            }
                        }
                    }
                    None
                }
            }));
        }

        for user in (join_all(handles).await).into_iter().flatten().flatten() {
            self.users.push(user);
        }
        progress_bar.finish_and_clear();
    }

    fn calculate_step_friendships(&self) -> usize {
        let total_users = self.users.len();
        let max_friendships = (total_users * (total_users - 1)) / 2;
        ((max_friendships as f32) * self.config.friendship_ratio).ceil() as usize
    }

    async fn init_friendships(&mut self) {
        let amount_of_friendships = self.calculate_step_friendships();

        let progress_bar = create_progress_bar(
            "Init friendhips".to_string(),
            (amount_of_friendships - self.friendships.len())
                .try_into()
                .unwrap(),
        );
        progress_bar.tick();

        let mut handles = vec![];
        while self.friendships.len() < amount_of_friendships {
            let mut rng = rand::thread_rng();
            let first_user = self.users.iter().choose(&mut rng).unwrap();
            let second_user = self.users.iter().choose(&mut rng).unwrap();
            if first_user.id() == second_user.id() {
                continue;
            }

            let friendship = Friendship::from_users(first_user, second_user);

            if self.friendships.contains(&friendship) {
                continue;
            }

            handles.push(tokio::spawn({
                self.friendships.push(friendship);
                let mut first_user = first_user.clone();
                let mut second_user = second_user.clone();
                let progress_bar = progress_bar.clone();
                async move {
                    let room_created = first_user.create_room().await;
                    if let Some(room_id) = room_created {
                        first_user.join_room(&room_id).await;
                        second_user.join_room(&room_id).await;
                    } else {
                        log::info!("User {} couldn't create a room", first_user.id());
                    }
                    progress_bar.inc(1);
                }
            }));
        }
        join_all(handles).await;
        progress_bar.finish_and_clear();
    }

    async fn act(&mut self) {
        let start = Instant::now();
        let step_secs = self.config.step_duration_in_secs;
        let step_duration = Duration::from_secs(step_secs as u64);

        let users_to_act = std::cmp::min(self.users.len(), self.config.max_users_to_act_per_tick);
        let progress_bar = create_progress_bar(
            "Running".to_string(),
            (step_secs * users_to_act).try_into().unwrap(),
        );
        progress_bar.tick();

        let mut one_sec_interval = interval(Duration::from_secs(1));
        let mut rng = rand::thread_rng();
        loop {
            if start.elapsed().ge(&step_duration) {
                break;
            }
            let mut handles = vec![];

            for user in self.users.iter().choose_multiple(&mut rng, users_to_act) {
                let user = user.clone();
                handles.push(tokio::spawn({
                    let mut user = user.clone();
                    let progress_bar = progress_bar.clone();
                    async move {
                        user.act().await;
                        progress_bar.inc(1);
                    }
                }));
            }
            join_all(handles).await;

            one_sec_interval.tick().await;
        }
        progress_bar.finish_and_clear();
    }

    async fn waiting_period(&self) {
        let spinner = ProgressBar::new_spinner()
            .with_style(
                ProgressStyle::default_spinner()
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
                    .template("{prefix:.bold.dim} {spinner} {wide_msg}"),
            )
            .with_prefix("Tear down:");

        let waiting_time = Duration::from_secs(self.config.waiting_period as u64);
        let one_sec = Duration::from_secs(1);
        let start = Instant::now();
        while !self.metrics.all_messages_received() {
            if start.elapsed().ge(&waiting_time) {
                break;
            }
            let wait_one_sec = Instant::now();
            spinner.set_message("Waiting for messages...");
            loop {
                if wait_one_sec.elapsed().ge(&one_sec) {
                    break;
                }
                sleep(Duration::from_millis(100));
                spinner.inc(1);
            }

            spinner.set_message("Checking all were messages received...");
        }
    }

    pub async fn run(&mut self) {
        println!("{:?}", self.config);
        for step in 1..=self.config.total_steps {
            println!("Running step {}", step);

            self.init_users().await;
            self.init_friendships().await;
            self.act().await;
            self.waiting_period().await;

            println!("{}", self);

            self.metrics.generate_report(self.config.output_dir.clone());
        }
    }
}
