use friendship::{Friendship, FriendshipID};
use matrix_sdk::ruma::{RoomId, UserId};
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use time::time_now;
use user::User;

mod friendship;
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
    users: HashMap<usize, User>,
    rooms: HashMap<UserId, Vec<RoomId>>,
}

impl Display for State {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        println!("Amount of users: {}", self.users.len());
        println!("Amount of friendships: {}", self.friendships.len());

        println!("Listing friendships: ");
        for friendship in &self.friendships {
            println!("- {}", friendship);
        }

        Ok(())
    }
}

impl State {
    pub fn new(config: Configuration) -> Self {
        Self {
            config,
            friendships: vec![],
            users: HashMap::new(),
            rooms: HashMap::new(),
        }
    }

    async fn init_users(&mut self, counter: &Arc<AtomicUsize>) {
        let timestamp = time_now();
        let actual_users = self.users.len();
        let desired_users = actual_users + self.config.users_per_step;

        let users_iter = actual_users..desired_users;

        let mut tasks = vec![];

        let server = self.config.homeserver_url.clone();
        for i in users_iter {
            tasks.push(tokio::spawn({
                let counter = counter.clone();
                let server = server.clone();
                async move {
                    let id = format!("user_{i}_{timestamp}");
                    let user = User::new(id, server).await;
                    match user {
                        Ok(user) => {
                            user.register().await;
                            user.login().await;
                            user.sync(&counter).await;
                            Ok((i, user))
                        }
                        Err(e) => Err(format!("Failed to create new user {} with error {}", i, e)),
                    }
                }
            }));
        }

        for result in (futures::future::join_all(tasks).await)
            .into_iter()
            .flatten()
        {
            match result {
                Ok((i, user)) => {
                    self.users.insert(i, user);
                }
                Err(e) => {
                    log::info!("{e}")
                }
            }
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

            let user1 = self.users.get(&random_user1).unwrap();
            let user2 = self.users.get(&random_user2).unwrap();

            let friendship = Friendship::from_users(user1, user2);

            if self.friendships.contains(&friendship) {
                continue;
            }

            self.friendships.push(friendship);

            let response = user1.create_room().await;
            match response {
                Ok(response) => {
                    let room_id = response.room_id;
                    user1.join_room(&room_id).await;
                    user2.join_room(&room_id).await;
                }
                Err(e) => {
                    // report error
                }
            }
        }
    }

    async fn send_messages(&mut self, counter: &Arc<AtomicUsize>) {
        for user in self.users.values() {
            if let Some(rooms) = self.rooms.get(&user.id()) {
                for room_id in rooms {
                    let counter_value = counter.load(Ordering::SeqCst) + 1;
                    counter.store(counter_value, Ordering::SeqCst);
                    log::info!("Current message counter: {}", counter_value);

                    match user.send_message(room_id).await {
                        Ok(_) => {}
                        Err(e) => {
                            // report send_message error
                        }
                    }
                }
            }
        }
    }

    async fn wait_for_messages(&mut self, counter: &Arc<AtomicUsize>) {
        loop {
            let counter_value = counter.load(Ordering::SeqCst);
            if counter_value == 0 {
                break;
            }
        }
    }

    pub async fn run(&mut self) {
        let counter = Arc::new(AtomicUsize::new(0));
        for step in 1..=self.config.total_steps {
            let now = Instant::now();
            println!("Starting step {}", step);

            let timer = Instant::now();
            print!("Initializing users...");
            io::stdout().flush().unwrap();

            self.init_users(&counter.clone()).await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            let timer = Instant::now();
            print!("Initializing friendships and rooms...");
            io::stdout().flush().unwrap();
            self.init_friendships().await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            let timer = Instant::now();
            print!("Sending messages...");
            io::stdout().flush().unwrap();
            self.send_messages(&counter.clone()).await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            let timer = Instant::now();
            print!("Waiting for all messages to sync...");
            io::stdout().flush().unwrap();
            self.wait_for_messages(&counter.clone()).await;
            println!("finished and took {} ms", timer.elapsed().as_millis());

            println!("{}", self);
            println!("Step finished in {} ms", now.elapsed().as_millis());
        }
    }
}
