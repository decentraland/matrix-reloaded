use std::path::Path;
use std::{collections::HashMap, fs::File, io::Write};

use matrix_sdk::ruma::exports::serde_json;
use matrix_sdk::ruma::RoomId;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;

#[derive(Serialize, Deserialize, PartialEq, Debug, Default, Clone)]
pub struct SavedUserState {
    pub available: i64,
    pub friendships: Vec<(usize, usize, Box<RoomId>)>,

    #[serde(skip)]
    pub friendships_by_user: HashMap<usize, Vec<usize>>,
}

impl SavedUserState {
    pub fn init_friendships(&mut self) {
        self.friendships_by_user = HashMap::new();

        for &(user1, user2, _) in &self.friendships {
            let user1_friends = self.friendships_by_user.entry(user1).or_insert(vec![]);
            user1_friends.push(user2);

            let user2_friends = self.friendships_by_user.entry(user2).or_insert(vec![]);
            user2_friends.push(user1);
        }
    }

    pub fn add_friendship(&mut self, user1: usize, user2: usize, room_id: Box<RoomId>) {
        let mut users = [user1, user2];
        users.sort_unstable();

        // This clause makes sure that a friendship is created only once, since they are bidirectional relations.
        // Since the users array is sorted when created, only checking one direction is enough
        if self
            .friendships
            .contains(&(users[0], users[1], room_id.clone()))
        {
            return;
        }

        self.friendships.push((users[0], users[1], room_id));
    }
}

#[serde_as]
#[derive(Deserialize, Serialize, PartialEq, Debug, Default)]
pub struct SavedUsers {
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    pub users: HashMap<String, SavedUserState>,
}

impl SavedUsers {
    pub fn get_available_users(&mut self, server: &str) -> &SavedUserState {
        let res = self.users.entry(server.to_string()).or_default();

        res.init_friendships();

        res
    }

    pub fn add_user(&mut self, key: String, value: SavedUserState) {
        self.users.insert(key, value);
    }
}

pub fn save_users(users: &SavedUsers, filename: String) {
    let str = serde_json::to_string(users).unwrap();

    let mut buffer = File::create(filename).unwrap();

    match buffer.write_all(str.as_bytes()) {
        Ok(_) => {}
        Err(err) => {
            println!("Failed to write the new users state {}", err)
        }
    }
}

pub fn load_users(file: String) -> SavedUsers {
    if !Path::new(&file).exists() {
        return SavedUsers {
            users: HashMap::new(),
        };
    }

    let file_content = std::fs::read_to_string(file).expect("cannot read file '{file}'");

    serde_json::from_str::<SavedUsers>(&file_content).expect("cannot parse file content '{file}'")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{load_users, save_users, SavedUserState, SavedUsers};

    #[test]
    fn identity_creation() {
        let mut saved_state = SavedUsers {
            users: HashMap::new(),
        };

        saved_state.users.insert(
            "Asd".to_string(),
            SavedUserState {
                available: 10,
                ..Default::default()
            },
        );
        saved_state.users.insert(
            "Bsd".to_string(),
            SavedUserState {
                available: 10,
                friendships: vec![],
                friendships_by_user: HashMap::new(),
            },
        );
        saved_state.users.insert(
            "Csd".to_string(),
            SavedUserState {
                available: 10,
                ..Default::default()
            },
        );

        save_users(&saved_state, "users_test.json".to_string());
        let res1 = load_users("users_test.json".to_string());

        assert_eq!(
            res1, saved_state,
            "The read of the file should be equal to the structure generated in code"
        );
    }
}
