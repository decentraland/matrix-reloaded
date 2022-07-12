use configuration::Config;
use events::Event;
use events::EventCollector;
use futures::{future::join_all, lock::Mutex};
use rand::prelude::IteratorRandom;
use report::Report;
use std::{
    collections::BTreeMap,
    ops::{Range, Sub},
    sync::Arc,
    time::Instant,
};
use tokio::{
    sync::mpsc::{self, Sender},
    task::JoinHandle,
    time::sleep,
};
use tokio_context::task::TaskController;
use user::User;

mod client;
pub mod configuration;
mod events;
mod report;
mod text;
mod time;
mod user;

enum Entity {
    Waiting,
    Ready { user: Arc<Mutex<User>> },
}

impl Entity {
    fn waiting() -> Self {
        Self::Waiting
    }

    fn from_user(user: User) -> Self {
        Self::Ready {
            user: Arc::new(Mutex::new(user)),
        }
    }
}
pub struct Simulation {
    config: Config,
    entities: BTreeMap<usize, Entity>,
}

impl Simulation {
    pub fn with_config(config: Config) -> Self {
        let entities = (0..config.simulation.max_users).fold(BTreeMap::new(), |mut map, i| {
            map.insert(i, Entity::waiting());
            map
        });
        Self { entities, config }
    }

    fn pick_random_entity(&mut self) -> (usize, &Entity) {
        let id = pick_random_id(0..self.config.simulation.max_users);
        (
            id,
            self.entities.get(&id).expect("entity should be present"),
        )
    }

    pub async fn run(&mut self) {
        // channel used to share events from users to the Event Collector
        let (tx, rx) = mpsc::channel::<Event>(100);

        // start collecting events in separated thread
        let event_collector = EventCollector::new();
        let events_report = event_collector.start(rx);

        // start simulation
        for tick_number in 0..self.config.simulation.ticks {
            self.tick(tick_number, &tx, &event_collector).await;
        }

        // notify simulation ended after a time period
        self.cool_down(&tx).await;

        // wait for report response
        let final_report = events_report.await.expect("events collection to end");

        self.store_report(&final_report).await;
    }

    async fn cool_down(&self, tx: &Sender<Event>) {
        // sleep main thread while missing messages are recevied
        sleep(self.config.simulation.grace_period_duration).await;

        // send finish event
        tx.send(Event::Finish).await.expect("channel open");
    }

    async fn tick(
        &mut self,
        tick_number: usize,
        tx: &Sender<Event>,
        event_collector: &EventCollector,
    ) {
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

        let tick_report = event_collector.snapshot_report().await;
        println!("tick {} report: {:#?}", tick_number, tick_report);

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
                    let mut user = user.lock().await;
                    user.act(&config).await;
                }
            }))
        } else {
            None
        }
    }

    pub async fn store_report(&self, report: &Report) {
        let output_folder = self.config.simulation.output.as_str();
        let homeserver = self.config.server.homeserver.as_str();

        let output_dir = format!("{output_folder}/{homeserver}");

        report.generate(output_dir.as_str());
    }
}

fn pick_random_id(range: Range<usize>) -> usize {
    let mut rng = rand::thread_rng();
    range.choose(&mut rng).unwrap()
}
