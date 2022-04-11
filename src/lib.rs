use friendship::{Friendship, FriendshipID};
use metrics::Metrics;
use rand::Rng;
use std::fmt::Display;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use time::time_now;
use user::{Synching, User};

mod friendship;
mod metrics;
mod text;
mod time;
mod user;

pub struct Configuration {
    pub total_steps: usize,
    pub users_per_step: usize,
    pub friendship_ratio: f32,
    pub homeserver_url: String,
}
pub struct State {
    config: Configuration,
    friendships: Vec<Friendship>,
    users: Vec<User<Synching>>,
    metrics: Arc<Mutex<Metrics>>,
}

impl Display for State {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        println!("Amount of users: {}", self.users.len());
        println!("Amount of friendships: {}", self.friendships.len());

        println!("Listing friendships: ");
        for friendship in &self.friendships {
            println!("- {}", friendship);
        }
        let metrics = self.metrics.lock().unwrap();

        println!("HTTP Errors count: {}", metrics.http_errors);
        println!("Sent messages count: {}", metrics.sent_messages.len());
        println!(
            "Recevied messages count: {}",
            metrics.received_messages.len()
        );

        Ok(())
    }
}

impl State {
    pub fn new(config: Configuration) -> Self {
        Self {
            config,
            friendships: vec![],
            users: vec![],
            metrics: Arc::new(Mutex::new(Metrics::default())),
        }
    }

    async fn init_users(&mut self) {
        let timestamp = time_now();
        let actual_users = self.users.len();
        let desired_users = actual_users + self.config.users_per_step;

        let users_iter = actual_users..desired_users;

        let mut tasks = vec![];

        let server = self.config.homeserver_url.clone();
        for i in users_iter {
            tasks.push(tokio::spawn({
                let metrics = self.metrics.clone();
                let server = server.clone();
                async move {
                    let id = format!("user_{i}_{timestamp}");
                    let user = User::new(id, server, metrics).await;

                    if let Some(user) = user {
                        if let Some(user) = user.register().await {
                            if let Some(user) = user.login().await {
                                log::info!("User is now synching: {}", user.id());
                                return Some(user.sync().await);
                            }
                        }
                    }
                    None
                }
            }));
        }

        for user in (futures::future::join_all(tasks).await)
            .into_iter()
            .flatten()
            .flatten()
        {
            self.users.push(user);
        }
    }

    async fn init_friendships(&mut self) {
        let amount_of_friendships =
            ((self.users.len() as f32) * self.config.friendship_ratio).ceil() as usize;
        let amount_of_users = self.users.len();

        while self.friendships.len() < amount_of_friendships {
            let random_user1 = rand::thread_rng().gen_range(0..amount_of_users);
            let random_user2 = rand::thread_rng().gen_range(0..amount_of_users);
            if random_user1 == random_user2 {
                continue;
            }

            let user1 = &self.users[random_user1];
            let user2 = &self.users[random_user2];

            let friendship = Friendship::from_users(user1, user2);

            if self.friendships.contains(&friendship) {
                continue;
            }

            self.friendships.push(friendship);

            let room_created = user1.create_room().await;
            if let Some(room_id) = room_created {
                self.users[random_user1].join_room(&room_id).await;
                self.users[random_user2].join_room(&room_id).await;
            } else {
                log::info!("User {} couldn't create a room", user1.id());
            }
        }
    }

    async fn act(&mut self) {
        for user in &self.users {
            user.act().await;
        }
    }

    pub async fn run(&mut self) {
        for step in 1..=self.config.total_steps {
            let now = Instant::now();
            println!("Starting step {}", step);

            let timer = Instant::now();
            print!("Initializing users...");
            io::stdout().flush().unwrap();

            self.init_users().await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            let timer = Instant::now();
            print!("Initializing friendships and rooms...");
            io::stdout().flush().unwrap();
            self.init_friendships().await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            self.act().await;

            let secs = Duration::from_secs(5);
            thread::sleep(secs);

            println!("{}", self);
            println!("Step finished in {} ms", now.elapsed().as_millis());
        }
    }
}
