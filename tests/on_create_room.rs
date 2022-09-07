use matrix_reloaded::{
    client::{Client, LoginResult, RegisterResult},
    events::{Event, UserNotifications},
};
use rand::{distributions::Alphanumeric, Rng};
use tokio::sync::mpsc;
mod config;

#[tokio::test]
async fn on_room_creation() {
    let (sender, _receiver) = mpsc::channel::<Event>(10);
    let config = config::create_test_config();

    // create client
    let client = Client::new(sender, &config).await;
    let user_localpart = "test_user";
    // regiter/login
    let result = client.register(user_localpart).await;
    match result {
        RegisterResult::Ok => {
            println!("User registered or already exists");
        }
        RegisterResult::Failed => panic!("user doesn't exist and couldn't register it"),
    }

    let result = client.login(user_localpart).await;
    match result {
        LoginResult::Failed => panic!("couldn't login user"),
        LoginResult::NotRegistered => panic!("user not registered, should never happen"),
        LoginResult::Ok => {}
    }
    // create user notifier channel to read room creation event
    let (user_notifications_sender, mut user_notifications_receiver) =
        mpsc::channel::<UserNotifications>(1);
    // sync
    let _sync_result = client.sync(&user_notifications_sender).await;
    // create room
    let mut channel_name: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();

    channel_name.push_str("-Pchannel");
    client.create_channel(channel_name).await;

    // read creation event
    while let Some(event) = user_notifications_receiver.recv().await {
        println!("Event received: {:#?}", event);
        if let UserNotifications::NewChannel(channel_id) = event {
            println!("Channel created! {}", channel_id);
            return;
        }
    }
    println!("No more events to read");
}
