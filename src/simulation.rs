use crate::configuration::Config;
use crate::events::Event;
use crate::events::EventCollector;
use crate::events::UserNotifications;
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
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::ruma::OwnedUserId;
use rand::prelude::IteratorRandom;
use std::collections::HashSet;
use std::time::Duration;
use std::{collections::BTreeMap, ops::Sub, sync::Arc, time::Instant};
use tokio::time::timeout;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
    time::sleep,
};

enum Entity {
    Waiting { id: usize },
    Ready { user: Arc<RwLock<User>> },
}

enum EntityAction {
    WakeUp(User),
    Act(JoinHandle<()>),
}

pub struct Context {
    pub syncing_users: RwLock<HashSet<OwnedUserId>>,
    pub config: Arc<Config>,
    notifier: Sender<Event>,
    pub user_notifier: Sender<UserNotifications>,
    pub channels: RwLock<HashSet<OwnedRoomId>>, // public channels created by all users
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

    async fn act(&self, context: Arc<Context>, time_to_act: Duration) -> EntityAction {
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
                        log::debug!("user locked {}", user.localpart);
                        if (timeout(time_to_act, user.act(&context)).await).is_err() {
                            log::debug!("user action took more than {:?}", time_to_act);
                        }
                        log::debug!("user unlocked {}", user.localpart);
                    }
                };
                let handle = tokio::spawn(action);
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

        let (user_notification_sender, user_notification_receiver) =
            mpsc::channel::<UserNotifications>(100);

        let context = Arc::new(Context {
            syncing_users: RwLock::new(HashSet::new()),
            config: self.config.clone(),
            notifier: tx.clone(),
            user_notifier: user_notification_sender.clone(),
            channels: RwLock::new(HashSet::new()),
        });

        tokio::spawn(Simulation::collect_user_notifications(
            user_notification_receiver,
            context.clone(),
        ));

        // start simulation
        for _ in 0..self.config.simulation.ticks {
            self.tick(context.clone()).await;
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

    async fn tick(&mut self, context: Arc<Context>) {
        let tick_start = Instant::now();
        let tick_duration = self.config.simulation.tick_duration;

        let mut join_handles = vec![];

        let user_ids = self.pick_users(self.config.simulation.users_per_tick);
        for user_id in user_ids {
            let entity = self.entities.get(&user_id).expect("user to exist");
            match entity.act(context.clone(), tick_duration).await {
                EntityAction::WakeUp(user) => {
                    self.entities.insert(user_id, Entity::from_user(user));
                }
                EntityAction::Act(user_action) => {
                    join_handles.push(user_action);
                }
            }
        }
        join_all(join_handles).await;

        if tick_start.elapsed().le(&tick_duration) {
            sleep(tick_duration.sub(tick_start.elapsed())).await;
        }
    }

    fn pick_users(&self, amount: usize) -> Vec<usize> {
        let mut rng = rand::thread_rng();

        (0..self.config.simulation.max_users).choose_multiple(&mut rng, amount)
    }

    async fn track_users(&mut self) {
        let syncing = self.get_syncing_users().await.len();
        self.progress.tick(syncing as u64);
    }

    async fn store_report(&self, report: &Report) {
        let output_folder = self.config.simulation.output.as_str();
        let homeserver = self.config.server.homeserver.as_str();

        let output_dir = format!("{output_folder}/{homeserver}");

        report.generate(output_dir.as_str(), &execution_id());
    }

    async fn get_syncing_users(&self) -> Vec<OwnedUserId> {
        let mut online_users = vec![];
        for (_, entity) in self.entities.iter() {
            if let Entity::Ready { user } = entity {
                if let Ok(user) = user.try_read() {
                    if let State::Sync { .. } = user.state {
                        let user_id = user.id().await.expect("user_id to be present");
                        online_users.push(user_id);
                    }
                }
            }
        }
        online_users
    }

    async fn collect_user_notifications(
        mut notification_receiver: Receiver<UserNotifications>,
        context: Arc<Context>,
    ) {
        log::debug!("spawning collect user notification");
        while let Some(event) = notification_receiver.recv().await {
            match event {
                UserNotifications::NewChannel(room_id) => {
                    log::debug!(
                        "collect_user_notifications event => {} data => {}",
                        "NEW CHANNEL",
                        room_id
                    );
                    context.channels.write().await.insert(room_id);
                }
                UserNotifications::NewSyncedUser(user_id) => {
                    log::debug!(
                        "collect_user_notifications event => {} data => {}",
                        "NEW SYNCED USER",
                        user_id
                    );
                    context.syncing_users.write().await.insert(user_id);
                }
                UserNotifications::UserLoggedOut(user_id) => {
                    log::debug!(
                        "collect_user_notifications event => {} data => {}",
                        "USER LOGGED OUT",
                        user_id
                    );
                    context.syncing_users.write().await.remove(&user_id);
                }
            }
        }
    }
}
