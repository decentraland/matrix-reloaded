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
    Waiting { id: usize },
    Ready { user: Arc<RwLock<User>> },
}

enum EntityAction {
    WakeUp(User),
    Act(JoinHandle<Option<()>>),
}

pub struct Context {
    pub syncing_users: Vec<usize>,
    pub config: Arc<Config>,
    notifier: Sender<Event>,
}

impl Entity {
    fn waiting(id: usize) -> Self {
        Self::Waiting { id }
    }

    fn from_user(user: User) -> Self {
        Self::Ready {
            user: Arc::new(RwLock::new(user)),
        }
    }

    async fn act(&self, context: Arc<Context>, controller: &mut TaskController) -> EntityAction {
        match &self {
            Entity::Waiting { id } => {
                log::debug!(" --- waking up entity {}", id);
                let user = User::new(*id, context.notifier.clone(), &context.config).await;
                EntityAction::WakeUp(user)
            }
            Entity::Ready { user } => {
                let action = {
                    let user = user.clone();
                    let context = context.clone();
                    async move {
                        let mut user = user.write().await;
                        log::debug!("user locked {}", user.id);
                        user.act(&context).await;
                        log::debug!("user unlocked {}", user.id);
                    }
                };
                let handle = controller.spawn(action);
                EntityAction::Act(handle)
            }
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
            map.insert(i, Entity::waiting(i));
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
            self.track_users();
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

        let user_ids = self.pick_users(self.config.simulation.users_per_tick);

        let context = Arc::new(Context {
            syncing_users: self.get_syncing_users(),
            config: self.config.clone(),
            notifier: tx.clone(),
        });

        for user_id in user_ids {
            let entity = self.entities.get(&user_id).expect("user to exist");
            match entity.act(context.clone(), &mut controller).await {
                EntityAction::WakeUp(user) => {
                    self.entities.insert(user_id, Entity::from_user(user));
                }
                EntityAction::Act(user_action) => {
                    join_handles.push(user_action);
                }
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

    fn track_users(&mut self) {
        let syncing = self.get_syncing_users().len();
        self.progress.tick(syncing as u64);
    }

    async fn store_report(&self, report: &Report) {
        let output_folder = self.config.simulation.output.as_str();
        let homeserver = self.config.server.homeserver.as_str();

        let output_dir = format!("{output_folder}/{homeserver}");

        report.generate(output_dir.as_str(), &execution_id());
    }

    fn get_syncing_users(&self) -> Vec<usize> {
        let mut online_users = vec![];
        for (id, entity) in self.entities.iter() {
            if let Entity::Ready { user } = entity {
                if let Ok(user) = user.try_read() {
                    if let State::Sync { .. } = user.state {
                        online_users.push(*id);
                    }
                }
            }
        }
        online_users
    }
}