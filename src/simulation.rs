use crate::configuration::Config;
use crate::events::Event;
use crate::events::EventCollector;
use crate::report::Report;
use crate::text::create_simulation_bar;
use crate::text::create_users_bar;
use crate::text::default_spinner;
use crate::text::spin_for;
use crate::user::State;
use crate::user::User;
use futures::future::join_all;
use indicatif::MultiProgress;
use indicatif::ProgressBar;
use matrix_sdk::locks::RwLock;
use rand::prelude::IteratorRandom;
use std::env;
use std::thread;
use std::{collections::BTreeMap, ops::Sub, sync::Arc, time::Instant};
use tokio::{
    sync::mpsc::{self, Sender},
    task::JoinHandle,
    time::sleep,
};
use tokio_context::task::TaskController;

enum Entity {
    Waiting,
    Ready { user: Arc<RwLock<User>> },
}

impl Entity {
    fn waiting() -> Self {
        Self::Waiting
    }

    fn from_user(user: User) -> Self {
        Self::Ready {
            user: Arc::new(RwLock::new(user)),
        }
    }
}
pub struct Simulation {
    config: Arc<Config>,
    entities: BTreeMap<usize, Entity>,
    progress: SimulationProgress,
}

impl Simulation {
    pub fn with_config(config: Config) -> Self {
        let entities = (0..config.simulation.max_users).fold(BTreeMap::new(), |mut map, i| {
            map.insert(i, Entity::waiting());
            map
        });
        let progress =
            SimulationProgress::new(config.simulation.ticks, config.simulation.max_users);
        Self {
            entities,
            config: Arc::new(config),
            progress,
        }
    }

    fn pick_random_entity(&mut self) -> (usize, &Entity) {
        let mut rng = rand::thread_rng();
        let id = (0..self.config.simulation.max_users)
            .choose(&mut rng)
            .unwrap();
        (
            id,
            self.entities.get(&id).expect("entity should be present"),
        )
    }

    pub async fn run(&mut self) {
        println!("server: {:#?}", self.config.server);
        println!("simulation config: {:#?}", self.config.simulation);

        self.progress.start();
        // channel used to share events from users to the Event Collector
        let (tx, rx) = mpsc::channel::<Event>(100);

        // start collecting events in separated thread
        let event_collector = EventCollector::new();
        let events_report = event_collector.start(rx);

        // start simulation
        for _ in 0..self.config.simulation.ticks {
            self.tick(&tx).await;
            self.track_users().await;
        }

        // notify simulation ended after a time period
        self.cool_down(&tx).await;
        self.progress.finish();

        // wait for report response
        let final_report = events_report.await.expect("events collection to end");

        self.store_report(&final_report).await;
    }

    async fn cool_down(&self, tx: &Sender<Event>) {
        let spinner = default_spinner();
        spinner.set_message("cool down: ");
        // sleep main thread while missing messages are recevied
        spin_for(self.config.simulation.grace_period_duration, &spinner).await;

        // send finish event
        tx.send(Event::Finish).await.expect("channel open");
    }

    async fn tick(&mut self, tx: &Sender<Event>) {
        let tick_start = Instant::now();
        let tick_duration = &mut self.config.simulation.tick_duration.clone();

        let mut controller = TaskController::with_timeout(*tick_duration);
        let mut join_handles = vec![];

        for _ in 0..self.config.simulation.users_per_tick {
            let user_action = self.pick_user_action(tx, &mut controller).await;
            if let Some(user_action) = user_action {
                join_handles.push(user_action);
            }
        }
        join_all(join_handles).await;

        if tick_start.elapsed().le(tick_duration) {
            sleep(tick_duration.sub(tick_start.elapsed())).await;
        }
    }

    async fn pick_user_action(
        &mut self,
        tx: &Sender<Event>,
        controller: &mut TaskController,
    ) -> Option<JoinHandle<Option<()>>> {
        let (id, entity) = self.pick_random_entity();
        if let Entity::Waiting = entity {
            log::debug!(" --- waking up entity {}", id);
            let user = User::new(id, tx.clone(), &self.config).await;
            self.entities.insert(id, Entity::from_user(user));
        }
        if let Entity::Ready { user } = self.entities.get(&id).unwrap() {
            // user action task to be executed in parallel
            Some(controller.spawn({
                let user = user.clone();
                let config = self.config.clone();
                async move {
                    let mut user = user.write().await;
                    user.act(&config).await;
                }
            }))
        } else {
            None
        }
    }

    async fn track_users(&self) {
        let mut syncing = 0;

        for entity in self.entities.values() {
            if let Entity::Ready { user } = entity {
                if let Ok(user) = user.try_read() {
                    if let State::Sync { .. } = user.state {
                        syncing += 1;
                    }
                }
            }
        }

        self.progress.tick(syncing);
    }

    async fn store_report(&self, report: &Report) {
        let output_folder = self.config.simulation.output.as_str();
        let homeserver = self.config.server.homeserver.as_str();

        let output_dir = format!("{output_folder}/{homeserver}");

        report.generate(output_dir.as_str());
    }
}

struct SimulationProgress {
    multi_progress: Arc<MultiProgress>,
    progress_bar: ProgressBar,
    users_bar: ProgressBar,
}

impl SimulationProgress {
    fn new(total_ticks: usize, max_users: usize) -> Self {
        let multi_progress = Arc::new(MultiProgress::new());
        let progress_bar = multi_progress.add(create_simulation_bar(total_ticks));
        let users_bar = multi_progress.add(create_users_bar(max_users));
        Self {
            multi_progress,
            progress_bar,
            users_bar,
        }
    }

    fn start(&self) {
        let is_ci = env::var("CI").is_ok();
        if !is_ci {
            let m = self.multi_progress.clone();
            thread::spawn(move || m.join_and_clear().unwrap());
        }
    }

    fn tick(&self, users_syncing: u64) {
        let is_ci = env::var("CI").is_ok();
        if is_ci {
            println!("users syncing: {users_syncing}");
        } else {
            self.progress_bar.inc(1);
            self.users_bar.set_position(users_syncing);
        }
    }

    fn finish(&self) {
        self.progress_bar.disable_steady_tick();
        self.users_bar.disable_steady_tick();

        let is_ci = env::var("CI").is_ok();
        if !is_ci {
            self.progress_bar
                .finish_with_message("Simulation finished!");

            self.users_bar.finish_and_clear();
            self.multi_progress.join_and_clear().unwrap();
        }
    }
}
