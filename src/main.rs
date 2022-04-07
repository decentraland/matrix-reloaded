use std::fmt::Display;

use rand::Rng;

struct User {
    id: String,
    client: Option<String>,
}

type Friendship = String;

trait FriendshipFormat {
    fn create(user_one: usize, user_two: usize) -> String;
}

impl FriendshipFormat for Friendship {
    fn create(user_one: usize, user_two: usize) -> String {
        let mut users = [user_one, user_two];
        users.sort();

        format!("{:?}", users)
    }
}

struct State {
    friendships: Vec<Friendship>,
    users: Vec<User>,
}

impl State {
    fn contains_friendship(&self, first_user: usize, second_user: usize) -> bool {
        let friendship = Friendship::create(first_user, second_user);
        self.friendships.contains(&friendship)
    }

    pub fn new() -> Self {
        Self {
            friendships: vec![],
            users: vec![],
        }
    }

    pub fn init_users(&mut self, user_count: usize) {
        for i in 0..user_count {
            let user = User {
                id: i.to_string(),
                client: None,
            };

            self.users.push(user);
        }
    }

    pub fn init_friendships(&mut self, friendship_ratio: f32) {
        let amount_of_friendships = ((self.users.len() as f32) * friendship_ratio).ceil() as usize;

        while self.friendships.len() < amount_of_friendships {
            let random_user1 = rand::thread_rng().gen_range(0..amount_of_friendships);
            let random_user2 = rand::thread_rng().gen_range(0..amount_of_friendships);

            if random_user1 != random_user2
                && !(self.contains_friendship(random_user1, random_user2))
            {
                self.friendships
                    .push(Friendship::create(random_user1, random_user2));
            }
        }
    }
}

impl Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

fn main() {
    let mut state = State::new();

    state.init_users(100);
    state.init_friendships(0.2);

    println!("Step 1: {}", state);
    
}
