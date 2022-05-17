use exponential_backoff::Backoff;
use futures::future::join_all;
use futures::stream::iter;
use futures::StreamExt;
use indicatif::ProgressBar;
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use text::get_random_string;
use tokio::sync::mpsc::{self, Sender};
use tokio::time::sleep;
use tokio_context::task::TaskController;

use events::Event;
use friendship::{Friendship, FriendshipID};
use metrics::Metrics;
use report::ReportManager;
use text::{create_progress_bar, default_spinner, spin_for};
use user::{create_desired_users, join_users_to_room, Registered, Retriable, Synching, User};
use users_state::{load_users, save_users, SavedUserState};

mod events;
mod friendship;
mod metrics;
mod report;
mod text;
mod time;
mod user;
mod users_state;

#[serde_as]
#[derive(Serialize, Deserialize, Debug)]
pub struct Configuration {
    pub user_count: i64,
    pub users_filename: String,
    pub create: bool,
    pub delete: bool,
    pub run: bool,
    homeserver_url: String,
    output_dir: String,
    total_steps: usize,
    users_per_step: usize,
    friendship_ratio: f32,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "step_duration_in_secs")]
    step_duration: Duration,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "tick_duration_in_secs")]
    tick_duration: Duration,
    max_users_to_act_per_tick_ratio: f64,
    waiting_period: usize,
    retry_request_config: bool,
    respect_login_well_known: bool,
    user_creation_retry_attempts: usize,
    room_creation_retry_attempts: usize,
    user_creation_throughput: usize,
    room_creation_throughput: usize,
    room_creation_max_resource_wait_attempts: usize,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "backoff_min_secs")]
    backoff_min: Duration,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "backoff_max_secs")]
    backoff_max: Duration,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "wait_between_steps_secs")]
    wait_between_steps: Duration,
}

pub struct State {
    config: Configuration,
    friendships: Vec<Friendship>,
    users: HashMap<String, User<Synching>>,
    users_state: SavedUserState,
    available_users: i64,
}

impl State {
    pub fn new(config: Configuration) -> Self {
        let server = config.homeserver_url.clone();

        let mut saved_users = load_users(config.users_filename.clone());
        let available_users = saved_users.get_available_users(&server);

        let users_state = available_users.clone();

        Self {
            config,
            friendships: vec![],
            users: HashMap::new(),
            available_users: users_state.available,
            users_state,
        }
    }

    async fn init_users(&mut self, tx: Sender<Event>) {
        let start = Instant::now();
        let actual_users = self.users.len();
        let desired_users = actual_users + self.config.users_per_step;
        let server = &self.config.homeserver_url;
        let retry_enabled = self.config.retry_request_config;
        let retry_attempts = self.config.user_creation_retry_attempts;

        let progress_bar = create_progress_bar(
            "Init users".to_string(),
            (desired_users - actual_users).try_into().unwrap(),
        );
        progress_bar.tick();

        let backoff = Backoff::new(
            retry_attempts as u32,
            self.config.backoff_min,
            self.config.backoff_max,
        );

        let mut futures = vec![];
        let mut i = actual_users;

        while i < desired_users {
            let users_index = self.users_state.available - self.available_users;

            let user_id = format!("user_{users_index}");
            futures.push(sync_user(
                server.clone(),
                user_id.clone(),
                progress_bar.clone(),
                tx.clone(),
                &backoff,
                retry_enabled,
            ));

            i += 1;
            self.available_users -= 1;
        }

        let stream_iter = futures::stream::iter(futures);
        let mut user_creations_buffer =
            stream_iter.buffer_unordered(self.config.user_creation_throughput);

        while let Some(user) = user_creations_buffer.next().await {
            if let Some(user) = user {
                self.users.insert(user.id().localpart().to_string(), user);
            } else {
                log::info!("couldn't login user");
            }
        }

        log::info!("init users took {} seconds", start.elapsed().as_secs());
        progress_bar.finish_and_clear();
    }

    fn calculate_step_friendships(&self) -> usize {
        let total_users = self.users.len();
        let max_friendships = (total_users * (total_users - 1)) / 2;
        ((max_friendships as f32) * self.config.friendship_ratio).ceil() as usize
    }

    async fn init_friendships(&mut self) {
        let amount_of_friendships = self.calculate_step_friendships();
        println!("Running test for {} friendships", amount_of_friendships);

        let available_friendships = self
            .users_state
            .friendships
            .iter()
            .filter(|(user1, user2)| {
                self.users.contains_key(user1) && self.users.contains_key(user2)
            })
            .collect::<Vec<_>>();

        println!(
            "Available friendships {}, creating new {} friendships",
            available_friendships.len(),
            amount_of_friendships - self.friendships.len()
        );

        let progress_bar = create_progress_bar(
            "Init friendships".to_string(),
            (amount_of_friendships - self.friendships.len())
                .try_into()
                .unwrap(),
        );
        progress_bar.tick();

        let mut futures = vec![];

        let mut used = 0;

        // Here the code obtains the friendships to be used during the test.
        // It first tries to reuse the previously created friendships, if there aren't more available
        // it creates new ones.
        // It's important to note that only creating new ones is async, otherwise it's just populating the friendships array
        while self.friendships.len() < amount_of_friendships {
            let friendship = if used < available_friendships.len() {
                let &(user1, user2) = &available_friendships[used];

                let first_user = self.users.get_mut(user1).unwrap();
                let homeserver = first_user.id().server_name().to_string();

                let friendship =
                    Friendship::from_ids(homeserver, user1.to_string(), user2.to_string());

                first_user.add_friendship(&friendship).await;

                let second_user = self.users.get_mut(user2).unwrap();
                second_user.add_friendship(&friendship).await;

                used += 1;
                progress_bar.inc(1);

                friendship
            } else {
                let (first_user, second_user) = self.get_random_friendship();
                let friendship = Friendship::from_users(first_user, second_user);

                futures.push(join_users_to_room(
                    first_user,
                    second_user,
                    friendship.clone(),
                    &progress_bar,
                    self.config.room_creation_retry_attempts,
                    self.config.room_creation_max_resource_wait_attempts,
                ));

                friendship
            };

            self.friendships.push(friendship);
        }

        let stream_iter = iter(futures);
        let mut buffered_iter = stream_iter.buffer_unordered(self.config.room_creation_throughput);

        while let Some(res) = buffered_iter.next().await {
            if let Some((user1, user2)) = res {
                self.users_state.add_friendship(user1, user2)
            }
        }

        self.friendships.sort();

        self.store_new_users_state();

        progress_bar.finish_and_clear();
    }

    fn store_new_users_state(&mut self) {
        let mut saved_users = load_users(self.config.users_filename.clone());
        let users_state = saved_users
            .users
            .entry(self.config.homeserver_url.clone())
            .or_default();
        users_state.available = self.users_state.available;
        users_state.friendships = self.users_state.friendships.clone();
        users_state.friendships_by_user = self.users_state.friendships_by_user.clone();
        save_users(&saved_users, self.config.users_filename.clone());
    }

    fn get_random_friendship(&self) -> (&User<Synching>, &User<Synching>) {
        loop {
            let mut rng = rand::thread_rng();
            let first_user = self.users.iter().choose(&mut rng).unwrap();
            let second_user = self.users.iter().choose(&mut rng).unwrap();

            if first_user.0 == second_user.0 {
                // cannot be friends with themselves
                continue;
            }
            let friendship = Friendship::from_users(first_user.1, second_user.1);
            if self.friendships.contains(&friendship) {
                // friendship already exists
                continue;
            }

            break (first_user.1, second_user.1);
        }
    }

    async fn act(&mut self, tx: Sender<Event>, step: usize) {
        let start = Instant::now();

        let users_to_act =
            ((self.users.len() as f64) * self.config.max_users_to_act_per_tick_ratio) as usize;
        let users_to_act = std::cmp::min(self.users.len(), users_to_act);
        let progress_bar = create_progress_bar(
            "Running",
            (self.config.step_duration.as_secs_f64() / self.config.tick_duration.as_secs_f64())
                .ceil() as u64,
        );

        progress_bar.tick();

        loop {
            if start.elapsed().ge(&self.config.step_duration) {
                // elapsed time for current step reached, breaking the loop and proceed to next step
                break;
            }
            let loop_start = Instant::now();

            let mut controller = TaskController::with_timeout(self.config.tick_duration);

            let mut handles = vec![];
            let users_list = self.get_users_with_friendship(users_to_act);

            for user in users_list {
                // Every spawn result in a tokio::select! with the future and the timeout
                handles.push(controller.spawn({
                    let mut user = user.clone();
                    async move {
                        let message = format!("step {} - {}", step, get_random_string());
                        user.act(message).await;
                    }
                }));
            }

            // Timeout is contemplated in this join_all because of the controller spawning tasks.
            join_all(handles).await;

            // If elapsed time of the current iteration is less than tick duration, we wait until that time.
            let elapsed = loop_start.elapsed();
            if elapsed.le(&self.config.tick_duration) {
                sleep(self.config.tick_duration - elapsed).await;
            }
            progress_bar.inc(1);
        }
        progress_bar.finish_and_clear();

        tx.send(Event::AllMessagesSent)
            .await
            .expect("AllMessagesSent event");
    }

    fn get_users_with_friendship(&self, users_to_act: usize) -> Vec<&User<Synching>> {
        self.users
            .iter()
            .map(|user| user.1)
            .filter(|user| user.has_friendships())
            .take(users_to_act)
            .collect::<Vec<_>>()
    }

    async fn waiting_period(&self, tx: Sender<Event>, metrics: &Metrics) {
        let spinner = default_spinner().with_prefix("Tear down:");

        let waiting_time = Duration::from_secs(self.config.waiting_period as u64);
        let one_sec = Duration::from_secs(1);
        let start = Instant::now();
        while !metrics.all_messages_received().await {
            if start.elapsed().ge(&waiting_time) {
                break;
            }
            spinner.set_message("Waiting for messages...");
            spin_for(one_sec, &spinner).await;

            spinner.set_message("Checking all messages were received...");
        }

        tx.send(Event::Finish).await.expect("Finish event sent");
    }

    pub async fn run(&mut self) {
        println!("{:#?}\n", self.config);

        let desired_users = self.config.users_per_step * self.config.total_steps;

        if (self.available_users as usize) < desired_users {
            panic!("There are only {} available users to run the test on this server, but for this test {} users are needed, please create more and try again", self.available_users, desired_users);
        }

        let report_manager = ReportManager::with_output_dir(self.config.output_dir.clone());

        println!("Execution id {}", &report_manager.execution_id);

        let spinner = default_spinner().with_message(format!(
            "Waiting {}s between steps",
            self.config.wait_between_steps.as_secs()
        ));

        let (tx, rx) = mpsc::channel::<Event>(100);
        let metrics = Metrics::new(rx);
        for step in 1..=self.config.total_steps {
            println!("Running step {} of {}", step, self.config.total_steps);

            let handle = metrics.run();

            // step warm up
            self.init_users(tx.clone()).await;
            self.init_friendships().await;

            // step running
            self.act(tx.clone(), step).await;
            self.waiting_period(tx.clone(), &metrics).await;

            // generate report
            let report = handle.await.expect("read events loop should end correctly");
            report_manager.generate_report(self, step, report);

            // wait in between steps
            spin_for(self.config.wait_between_steps, &spinner).await;
        }
    }

    pub async fn create_users(&mut self) {
        let (tx, rx) = mpsc::channel::<Event>(100);
        let metrics = Metrics::new(rx);
        let handle = metrics.run();

        create_desired_users(&self.config, tx.clone()).await;

        let report = handle.await.expect("read events loop should end correctly");
        println!(
            "Create users command finished {} errors",
            if report.any_error() {
                "with"
            } else {
                "without"
            }
        )
    }
}

async fn sync_user(
    server: String,
    id: String,
    progress_bar: ProgressBar,
    tx: Sender<Event>,
    backoff: &Backoff,
    retry_enabled: bool,
) -> Option<User<Synching>> {
    let mut total_duration = Duration::from_micros(0);

    let mut error = None;

    for duration in backoff {
        total_duration += duration;

        let user = User::<Registered>::new(&id, &server, retry_enabled, tx.clone()).await;

        if let Some(mut user) = user {
            match user.login().await {
                Ok(user) => {
                    log::info!("User is now synching: {}", user.id());
                    progress_bar.inc(1);

                    return Some(user.sync().await);
                }
                Err((retriable, e)) => {
                    error = Some(e);
                    if let Retriable::NonRetriable = retriable {
                        // error is non retriable, breaking the retry loop
                        break;
                    }
                    sleep(duration).await;
                }
            }
        }
    }

    panic!(
        "Couldn't init user {} after a duration of {}s with error {}",
        &id,
        total_duration.as_secs(),
        error.unwrap()
    );
}
