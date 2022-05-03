use std::cmp::Ordering;

use crate::user::User;

#[derive(Clone, Eq)]
pub struct Friendship {
    pub full_string: String,
    pub local_part: String,
    pub homeserver: String,
}

impl PartialEq for Friendship {
    fn eq(&self, other: &Self) -> bool {
        self.full_string == other.full_string
    }
}

impl PartialOrd for Friendship {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Friendship {
    fn cmp(&self, other: &Self) -> Ordering {
        self.full_string.cmp(&other.full_string)
    }
}

pub trait FriendshipID {
    fn from_users<State>(user_one: &User<State>, user_two: &User<State>) -> Friendship;

    fn from_ids(homeserver: String, user_one: String, user_two: String) -> Friendship;
}

fn get_local_part(first: &str, second: &str) -> String {
    format!("{first}-{second}")
}

impl FriendshipID for Friendship {
    fn from_users<State>(user_one: &User<State>, user_two: &User<State>) -> Friendship {
        let id = user_one.id();
        let homeserver = id.server_name();
        let user_one = id.localpart().to_string();
        let user_two = user_two.id().localpart().to_string();
        let mut users = [user_one, user_two];
        users.sort_unstable();
        let first = &users[0];
        let second = &users[1];
        let local_part = get_local_part(first, second);

        Friendship {
            full_string: format!("@{local_part}:{homeserver}"),
            homeserver: homeserver.to_string(),
            local_part,
        }
    }

    fn from_ids(homeserver: String, user_one: String, user_two: String) -> Friendship {
        let mut users = [user_one, user_two];
        users.sort_unstable();
        let first = &users[0];
        let second = &users[1];

        let local_part = get_local_part(first, second);

        Friendship {
            full_string: format!("@{local_part}:{homeserver}"),
            homeserver: homeserver.to_string(),
            local_part,
        }
    }
}
