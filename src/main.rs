use matrix_sdk::room::Room;
use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageEventContent, SyncMessageEvent};
use matrix_sdk::ruma::{assign, RoomId};
use matrix_sdk::SyncSettings;
use matrix_sdk::{ruma::UserId, Client};
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use text::get_random_string;

mod text;

const SERVER: &str = "matrix.decentraland.zone";
const PASSWORD: &str = "asdfasdf";

struct User {
    id: UserId,
    client: Client,
}

impl User {
    pub async fn new(
        id: String,
        counter: &Arc<AtomicUsize>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // init client connection (register + login)
        let user_id = UserId::try_from(format!("@{id}:{SERVER}")).unwrap();

        // in case we are testing against localhost or not https server, we need to setup test cfg, see `Client::homeserver_from_user_id`
        let client = Client::new_from_user_id(user_id.clone())
            .await
            .expect("Couldn't create new client");

        let req = assign!(register::Request::new(), {
            username: Some(user_id.localpart()),
            password: Some(PASSWORD),
            auth: Some(AuthData::Dummy(Dummy::new()))
        });

        client.register(req).await.unwrap();

        client
            .login(user_id.localpart(), PASSWORD, None, None)
            .await
            .unwrap();

        tokio::spawn({
            let client = client.clone();
            async move {
                client.sync(SyncSettings::default()).await;
            }
        });

        client
            .register_event_handler({
                let counter = counter.clone();
                let user_id = user_id.clone();
                move |ev: SyncMessageEvent<MessageEventContent>, room: Room| {
                    let counter = counter.clone();
                    let user_id = user_id.clone();
                    async move {
                        if ev.sender.localpart() == user_id.localpart() {
                            return;
                        }
                        println!(
                            "User {} received a message from room {} and sent by {}",
                            user_id,
                            room.room_id(),
                            ev.sender
                        );
                        let actual_value = counter.load(Ordering::SeqCst);
                        println!("Messages counter before decrease: {}", actual_value);

                        counter.store(actual_value - 1, Ordering::SeqCst);
                        println!("Messages counter after decrease: {}", actual_value - 1);
                    }
                }
            })
            .await;

        Ok(Self {
            id: user_id,
            client,
        })
    }

    pub async fn send_message(&self, room_id: &RoomId) -> String {
        let content = AnyMessageEventContent::RoomMessage(MessageEventContent::text_plain(
            get_random_string(),
        ));

        let sent_message = self
            .client
            .room_send(room_id, content, None)
            .await
            .unwrap_or_else(|_| panic!("Couldn't send message to room {room_id}"));

        let event_id = sent_message.event_id.to_string();
        println!(
            "Message sent from {} to room {} with event id {}",
            self.id, room_id, event_id
        );
        event_id
    }
}

type Friendship = String;

trait FriendshipID {
    fn from_users(user_one: &User, user_two: &User) -> String;
}

impl FriendshipID for Friendship {
    fn from_users(user_one: &User, user_two: &User) -> String {
        let user_one = user_one.id.localpart().to_string();
        let user_two = user_two.id.localpart().to_string();
        let mut users = [user_one, user_two];
        users.sort_unstable();
        let first = &users[0];
        let second = &users[1];

        format!("@{first}-{second}:{SERVER}")
    }
}

#[derive(Default)]
struct Step {
    number: usize,
    counter: Arc<AtomicUsize>,
}

struct Configuration {
    total_steps: usize,
    users_per_step: usize,
    friendship_ratio: f32,
}
struct State {
    config: Configuration,
    current_step: Step,
    friendships: Vec<Friendship>,
    users: HashMap<usize, User>,
    rooms: HashMap<UserId, Vec<RoomId>>,
}

impl State {
    pub fn new(config: Configuration) -> Self {
        Self {
            config,
            current_step: Step {
                number: 0,
                counter: Arc::new(AtomicUsize::new(0)),
            },
            friendships: vec![],
            users: HashMap::new(),
            rooms: HashMap::new(),
        }
    }

    pub async fn init_users(&mut self) {
        let timestamp = time_now();
        let actual_users = self.users.len();
        let desired_users = actual_users + self.config.users_per_step;

        for i in actual_users..desired_users {
            let id = format!("user_{i}_{timestamp}");
            let user = User::new(id, &self.current_step.counter).await.unwrap();
            self.users.insert(i, user);
        }
    }

    pub async fn init_friendships(&mut self) {
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

            let request = create_room::Request::new();
            let room_id_response = user1.client.create_room(request).await.unwrap();
            let room_id = room_id_response.room_id;

            user1.client.join_room_by_id(&room_id).await.unwrap();
            self.rooms
                .entry(user1.id.clone())
                .or_default()
                .push(room_id.clone());

            user2.client.join_room_by_id(&room_id).await.unwrap();
            self.rooms
                .entry(user2.id.clone())
                .or_default()
                .push(room_id.clone());
        }
    }

    async fn send_messages(&mut self) {
        for user in self.users.values() {
            if let Some(rooms) = self.rooms.get(&user.id) {
                for room_id in rooms {
                    let counter = self.current_step.counter.load(Ordering::SeqCst) + 1;
                    self.current_step.counter.store(counter, Ordering::SeqCst);
                    println!("Current message counter: {}", counter);

                    user.send_message(room_id).await;
                }
            }
        }
    }

    async fn wait_for_messages(&mut self) {
        loop {
            let counter = self.current_step.counter.load(Ordering::SeqCst);
            if counter == 0 {
                break;
            }
        }
    }

    pub async fn run(&mut self) {
        let message_counter = Arc::new(AtomicUsize::new(0));
        for step in 1..=self.config.total_steps {
            let now = Instant::now();
            println!("Starting step {}", step);
            self.current_step = Step {
                number: step,
                counter: message_counter.clone(),
            };

            println!("Initializing users...");
            self.init_users().await;

            println!("Initializing friendships and rooms...");
            self.init_friendships().await;

            println!("Sending messages...");
            self.send_messages().await;

            println!("Waiting for all messages to sync...");
            self.wait_for_messages().await;

            println!("{}", self);
            println!("Step finished in {} ms", now.elapsed().as_millis());
        }
    }
}

fn time_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is valid")
        .as_millis()
}

impl Display for State {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        println!("Step: {}", self.current_step.number);
        println!("Amount of users: {}", self.users.len());
        println!("Amount of friendships: {}", self.friendships.len());

        println!("Listing friendships: ");
        for friendship in &self.friendships {
            println!("- {}", friendship);
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Configuration {
        total_steps: 2,
        users_per_step: 50,
        friendship_ratio: 0.5,
    };
    let mut state = State::new(config);
    state.run().await;

    Ok(())
}
