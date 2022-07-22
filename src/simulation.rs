use crate::configuration::Config;
use crate::events::Event;
use crate::events::EventCollector;
use crate::progress::create_progress;
use crate::progress::Progress;
use crate::report::Report;
use crate::text::default_spinner;
use crate::text::spin_for;
use crate::time::execution_id;
use crate::user::State;
use crate::user::User;
use futures::future::join_all;
use matrix_sdk::locks::RwLock;
use rand::prelude::IteratorRandom;
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
    progress: Box<dyn Progress>,
}

impl Simulation {
    pub fn with(config: Config) -> Self {
        let entities = (0..config.simulation.max_users).fold(BTreeMap::new(), |mut map, i| {
            map.insert(i, Entity::waiting());
            map
        });

        Self {
            entities,
            progress: create_progress(config.simulation.ticks, config.simulation.max_users),
            config: Arc::new(config),
        }
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

        let users = self.pick_users(self.config.simulation.users_per_tick);

        for user in users {
            let user_action = self.spawn_user_action(tx, &mut controller, user).await;
            if let Some(user_action) = user_action {
                join_handles.push(user_action);
            }
        }
        join_all(join_handles).await;

        if tick_start.elapsed().le(tick_duration) {
            sleep(tick_duration.sub(tick_start.elapsed())).await;
        }
    }

    fn pick_users(&self, amount: usize) -> Vec<usize> {
        let mut rng = rand::thread_rng();
        (0..self.config.simulation.max_users).choose_multiple(&mut rng, amount)
    }

    async fn spawn_user_action(
        &mut self,
        tx: &Sender<Event>,
        controller: &mut TaskController,
        user_id: usize,
    ) -> Option<JoinHandle<Option<()>>> {
        let entity = self
            .entities
            .get(&user_id)
            .expect("entity should be present");
        if let Entity::Waiting = entity {
            log::debug!(" --- waking up entity {}", user_id);
            let user = User::new(user_id, tx.clone(), &self.config).await;
            self.entities.insert(user_id, Entity::from_user(user));
        }
        if let Entity::Ready { user } = self.entities.get(&user_id).unwrap() {
            // user action task to be executed in parallel
            Some(controller.spawn({
                let user = user.clone();
                let config = self.config.clone();
                async move {
                    let mut user = user.write().await;
                    log::debug!("user locked {}", user_id);
                    user.act(&config).await;
                    log::debug!("user unlocked {}", user_id);
                }
            }))
        } else {
            None
        }
    }

    async fn track_users(&mut self) {
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

        report.generate(output_dir.as_str(), &execution_id());
    }
}
