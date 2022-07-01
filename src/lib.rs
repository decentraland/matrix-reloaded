use configuration::SimulationConfig;
use events::Event;
use futures::{future::join_all, lock::Mutex};
use metrics::Metrics;
use rand::prelude::IteratorRandom;
use std::{
    collections::BTreeMap,
    ops::{Range, Sub},
    sync::Arc,
    time::Instant,
};
use tokio::{sync::mpsc, time::sleep};
use tokio_context::task::TaskController;
use user::User;

mod client;
pub mod configuration;
mod events;
mod metrics;
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
    config: SimulationConfig,
    entities: BTreeMap<usize, Entity>,
}

impl Simulation {
    pub fn with_config(config: SimulationConfig) -> Self {
        Self {
            entities: (0..config.simulation.max_users).fold(BTreeMap::new(), |mut map, i| {
                map.insert(i, Entity::waiting());
                map
            }),
            config,
        }
    }

    fn pick_random_entity(&mut self) -> (usize, &Entity) {
        let id = pick_random_id(0..self.config.simulation.max_users);
        (
            id,
            self.entities.get(&id).expect("entity should be present"),
        )
    }

    pub async fn run(&mut self) {
        // channel used to share events from users to Metrics
        let (tx, rx) = mpsc::channel::<Event>(100);

        // start collecting metrics in separated thread
        let metrics = Metrics::new(rx);
        metrics.run();

        log::info!("starting simulation...");
        for tick in 0..self.config.simulation.ticks {
            log::info!(" - tick {tick}");
            let tick_duration = &mut self.config.simulation.tick_duration.clone();
            let mut controller = TaskController::with_timeout(*tick_duration);
            let tick_start = Instant::now();
            let mut join_handles = vec![];

            for tick_user in 0..self.config.simulation.users_per_tick {
                log::info!(" -- tick user {tick_user}");

                let (id, entity) = self.pick_random_entity();

                if let Entity::Waiting = entity {
                    log::info!(" --- waking up entity {}", id);
                    let user = User::new(id, tx.clone(), &self.config).await;
                    self.entities.insert(id, Entity::from_user(user));
                }

                if let Entity::Ready { user } = self.entities.get(&id).unwrap() {
                    // add user action task to be executed in parallel
                    join_handles.push(controller.spawn({
                        let user = user.clone();
                        let config = self.config.clone();
                        async move {
                            let mut user = user.lock().await;
                            user.act(&config).await;
                        }
                    }));
                }
            }

            join_all(join_handles).await;

            if tick_start
                .elapsed()
                .le(&self.config.simulation.tick_duration)
            {
                sleep(
                    self.config
                        .simulation
                        .tick_duration
                        .sub(tick_start.elapsed()),
                )
                .await;
            }
        }

        self.report(&metrics).await;
    }

    pub async fn report(&self, metrics: &Metrics) {}
}

fn pick_random_id(range: Range<usize>) -> usize {
    let mut rng = rand::thread_rng();
    range.choose(&mut rng).unwrap()
}
