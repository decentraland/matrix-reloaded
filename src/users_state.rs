use std::{collections::HashMap, fmt, fs::File, io::Write, marker::PhantomData};

use matrix_sdk::ruma::api::exports::serde_json;
use serde::{
    de::{MapAccess, Visitor},
    ser::SerializeMap,
    Deserialize, Deserializer, Serialize, Serializer,
};

#[derive(Serialize, Deserialize)]
pub struct SavedUserState {
    pub homeserver_url: String,
    pub amount: i64,
    pub friendships: Vec<(usize, usize)>,
}

pub struct SavedUsers {
    pub users: HashMap<u128, SavedUserState>,
}

impl SavedUsers {
    pub fn get_available_user_count(&self) -> i64 {
        let mut available_users: i64 = 0;

        for state in self.users.values() {
            available_users += state.amount;
        }

        available_users
    }

    pub fn add_user(&mut self, key: u128, value: SavedUserState) {
        self.users.insert(key, value);
    }
}

impl Serialize for SavedUsers {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.users.len()))?;
        for (k, v) in &self.users {
            map.serialize_entry(&k.to_string(), &v)?;
        }
        map.end()
    }
}

struct SavedUsersVisitor {
    marker: PhantomData<fn() -> SavedUsers>,
}

impl SavedUsersVisitor {
    fn new() -> Self {
        SavedUsersVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de> Visitor<'de> for SavedUsersVisitor {
    // The type that our Visitor is going to produce.
    type Value = SavedUsers;

    // Format a message stating what data this Visitor expects to receive.
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a very special map")
    }

    // Deserialize MyMap from an abstract "map" provided by the
    // Deserializer. The MapAccess input is a callback provided by
    // the Deserializer to let us see each entry in the map.
    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut map = HashMap::new();

        // While there are entries remaining in the input, add them
        // into our map.
        while let Some((key, value)) = access.next_entry()? {
            map.insert(key, value);
        }

        Ok(SavedUsers { users: map })
    }
}

impl<'de> Deserialize<'de> for SavedUsers {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Instantiate our Visitor and ask the Deserializer to drive
        // it over the input data, resulting in an instance of MyMap.
        let res = deserializer.deserialize_map(SavedUsersVisitor::new());

        match res {
            Ok(val) => Ok(val),
            Err(err) => {
                println!("Failed while deserializing saved state {}", err);
                Err(err)
            }
        }
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
    let file_content = std::fs::read_to_string(file).unwrap();
    let res: SavedUsers = serde_json::from_str(&file_content).unwrap();

    res
}
