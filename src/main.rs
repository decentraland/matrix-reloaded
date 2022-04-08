use matrix_sdk::ruma::api::client::r0::account::register;
use matrix_sdk::ruma::api::client::r0::room::create_room;
use matrix_sdk::{ruma::UserId, Client};
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Display;

const SERVER: &str = "matrix.decentraland.zone";

struct User {
    id: String,
    client: Client,
}

impl User {
    pub async fn new(id: String) -> Result<Self, Box<dyn std::error::Error>> {
        // init client connection (register + login)
        let user_id = UserId::try_from(format!("@{id}:{SERVER}")).unwrap();
        let client = Client::new_from_user_id(user_id.clone())
            .await
            .expect("Couldn't create new client");

        let registration = register::Request::new();
        client
            .register(registration)
            .await
            .expect("Couldn't register new user {user_id}");
        client
            .login(user_id.localpart(), "password", None, None)
            .await
            .expect("Couldn't login new user {user_id}");

        Ok(Self { id, client })
    }
}

type Friendship = String;

trait FriendshipFormat {
    fn create(user_one: usize, user_two: usize) -> String;
}

impl FriendshipFormat for Friendship {
    fn create(user_one: usize, user_two: usize) -> String {
        let mut users = [user_one, user_two];
        users.sort_unstable();

        format!("{:?}", users)
    }
}

struct State {
    friendships: Vec<Friendship>,
    users: HashMap<usize, User>,
}

impl State {
    fn contains_friendship(&self, first_user: usize, second_user: usize) -> bool {
        let friendship = Friendship::create(first_user, second_user);
        self.friendships.contains(&friendship)
    }

    pub fn new() -> Self {
        Self {
            friendships: vec![],
            users: HashMap::new(),
        }
    }

    pub async fn init_users(&mut self, user_count: usize) {
        for i in 0..user_count {
            let user = User::new(i.to_string()).await.unwrap();
            self.users.insert(i, user);
        }
    }

    pub async fn init_friendships(&mut self, friendship_ratio: f32) {
        let amount_of_friendships = ((self.users.len() as f32) * friendship_ratio).ceil() as usize;

        while self.friendships.len() < amount_of_friendships {
            let random_user1 = rand::thread_rng().gen_range(0..=amount_of_friendships);
            let random_user2 = rand::thread_rng().gen_range(0..=amount_of_friendships);

            if random_user1 == random_user2
                || (self.contains_friendship(random_user1, random_user2))
            {
                continue;
            }

            self.friendships
                .push(Friendship::create(random_user1, random_user2));

            let user1 = self.users.get(&random_user1).unwrap();
            let user2 = self.users.get(&random_user2).unwrap();

            let request = create_room::Request::new();

            let room_id_response = user1.client.create_room(request).await.unwrap();

            user1
                .client
                .join_room_by_id(&room_id_response.room_id)
                .await
                .unwrap();

            user2
                .client
                .join_room_by_id(&room_id_response.room_id)
                .await
                .unwrap();
        }
    }
}

impl Display for State {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        println!("Current state is: ");
        println!("Amount of users: {}", self.users.len());
        println!("Amount of friendships: {}", self.friendships.len());

        println!("Listing friendships: ");
        for friendship in &self.friendships {
            println!("{}", friendship);
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = State::new();
    tokio::spawn(async move {
        state.init_users(2).await;
        state.init_friendships(0.5).await;

        println!("Step 1: {}", state);
    })
    .await
    .unwrap();

    Ok(())
}
