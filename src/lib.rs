use friendship::{Friendship, FriendshipID};
use futures::future::join_all;
use metrics::Metrics;
use rand::Rng;
use std::fmt::Display;
use std::thread;
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

pub struct Configuration {
    pub homeserver_url: String,
    pub total_steps: usize,
    pub users_per_step: usize,
    pub friendship_ratio: f32,
    pub time_to_run_per_step: usize,
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
        let amount_of_users = self.users.len();
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
            let first_random_user = rand::thread_rng().gen_range(0..amount_of_users);
            let second_random_user = rand::thread_rng().gen_range(0..amount_of_users);
            if first_random_user == second_random_user {
                continue;
            }

            let first_user = &self.users[first_random_user];
            let second_user = &self.users[second_random_user];

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
        let secs = self.config.time_to_run_per_step;
        let time_to_run = Duration::from_secs(secs.try_into().unwrap());

        let progress_bar = create_progress_bar(
            "Running".to_string(),
            (secs * self.users.len()).try_into().unwrap(),
        );
        progress_bar.tick();

        let mut interval = interval(Duration::from_secs(1));

        loop {
            let duration_since_start = Instant::now().checked_duration_since(start).unwrap();
            if duration_since_start.ge(&time_to_run) {
                break;
            }

            interval.tick().await;
            let mut handles = vec![];

            for user in self.users.iter().take(100) {
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
        }
        progress_bar.finish_and_clear();
    }

    pub async fn run(&mut self) {
        for step in 1..=self.config.total_steps {
            println!("Running step {}", step);

            self.init_users().await;
            self.init_friendships().await;
            self.act().await;

            let secs = Duration::from_secs(5);
            thread::sleep(secs);
            println!("{}", self);

            self.metrics.generate_report();
        }
    }
}
