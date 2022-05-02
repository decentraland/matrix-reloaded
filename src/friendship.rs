use crate::user::User;

pub type Friendship = String;
pub trait FriendshipID {
    fn from_users<State>(user_one: &User<State>, user_two: &User<State>) -> String;

    fn from_ids(homeserver: String, user_one: String, user_two: String) -> String;
}

impl FriendshipID for Friendship {
    fn from_users<State>(user_one: &User<State>, user_two: &User<State>) -> String {
        let id = user_one.id();
        let homeserver = id.server_name();
        let user_one = id.localpart().to_string();
        let user_two = user_two.id().localpart().to_string();
        let mut users = [user_one, user_two];
        users.sort_unstable();
        let first = &users[0];
        let second = &users[1];

        format!("@{first}-{second}:{homeserver}")
    }

    fn from_ids(homeserver: String, user_one: String, user_two: String) -> String {
        let mut users = [user_one, user_two];
        users.sort_unstable();
        let first = &users[0];
        let second = &users[1];

        format!("@{first}-{second}:{homeserver}")
    }
}
