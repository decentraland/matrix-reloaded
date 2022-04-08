use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::ruma::api::client::r0::uiaa::{AuthData, Dummy};
use matrix_sdk::ruma::events::room::message::MessageEventContent;
use matrix_sdk::ruma::events::AnyMessageEventContent;
use matrix_sdk::ruma::{assign, RoomId};
use matrix_sdk::{ruma::UserId, Client};
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};

const SERVER: &str = "matrix.decentraland.zone";
const PASSWORD: &str = "asdfasdf";

struct User {
    id: UserId,
    client: Client,
}

impl User {
    pub async fn new(id: String) -> Result<Self, Box<dyn std::error::Error>> {
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

        Ok(Self {
            id: user_id,
            client,
        })
    }

    pub async fn send_message(&self, room_id: &RoomId) {
        let content =
            AnyMessageEventContent::RoomMessage(MessageEventContent::text_plain("Hello world"));

        self.client.room_send(room_id, content, None).await.unwrap();
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

struct State {
    friendships: Vec<Friendship>,
    users: HashMap<usize, User>,
}

impl State {
    pub fn new() -> Self {
        Self {
            friendships: vec![],
            users: HashMap::new(),
        }
    }

    pub async fn init_users(&mut self, user_count: usize) {
        let timestamp = time_now();

        for i in 0..user_count {
            let id = format!("user_{i}_{timestamp}");
            let user = User::new(id).await.unwrap();
            self.users.insert(i, user);
        }
    }

    pub async fn init_friendships(&mut self, friendship_ratio: f32) {
        let amount_of_friendships = ((self.users.len() as f32) * friendship_ratio).ceil() as usize;
        let amount_of_users = self.users.len();

        while self.friendships.len() < amount_of_friendships {
            let random_user1 = rand::thread_rng().gen_range(0..amount_of_users);
            let random_user2 = rand::thread_rng().gen_range(0..amount_of_users);

            let user1 = self.users.get(&random_user1).unwrap();
            let user2 = self.users.get(&random_user2).unwrap();

            let friendship = Friendship::from_users(user1, user2);

            if self.friendships.contains(&friendship) {
                continue;
            }

            self.friendships.push(friendship);

            let request = create_room::Request::new();

            println!("Creating room ");

            let room_id_response = user1.client.create_room(request).await.unwrap();

            println!("Created room {}", &room_id_response.room_id);

            let room_id = room_id_response.room_id;

            user1.client.join_room_by_id(&room_id).await.unwrap();
            user2.client.join_room_by_id(&room_id).await.unwrap();
        }
    }
}

fn time_now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is valid")
        .as_millis()
        .to_string()
}

impl Display for State {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        println!("Current state is: ");
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
    let mut state = State::new();

    state.init_users(2).await;

    tokio::spawn(async move {
        state.init_friendships(0.5).await;

        println!("Step 1: {}", state);
    })
    .await
    .unwrap();

    Ok(())
}
