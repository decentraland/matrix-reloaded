use std::path::Path;
use std::{collections::HashMap, fs::File, io::Write};

use matrix_sdk::ruma::exports::serde_json;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct SavedUserState {
    pub available: i64,
    pub friendships: Vec<(usize, usize)>,
}

#[serde_as]
#[derive(Deserialize, Serialize, PartialEq, Debug)]
pub struct SavedUsers {
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    pub users: HashMap<String, SavedUserState>,
}

impl SavedUsers {
    pub fn get_available_users(&self, server: String) -> Option<&SavedUserState> {
        self.users.get(&server)
    }

    pub fn add_user(&mut self, key: String, value: SavedUserState) {
        self.users.insert(key, value);
    }
}

pub fn save_users(users: &SavedUsers, filename: String) {
    let str = serde_json::to_string_pretty(users).unwrap();

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

    let file_content = std::fs::read_to_string(file).unwrap();

    let res: SavedUsers = serde_json::from_str(&file_content).unwrap();

    res
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
                friendships: vec![],
            },
        );
        saved_state.users.insert(
            "Bsd".to_string(),
            SavedUserState {
                available: 10,
                friendships: vec![],
            },
        );
        saved_state.users.insert(
            "Csd".to_string(),
            SavedUserState {
                available: 10,
                friendships: vec![],
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
