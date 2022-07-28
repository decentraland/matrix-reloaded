use bichannel::Channel;
use futures::Future;
use matrix_sdk::{config::SyncSettings, deserialized_responses::SyncResponse, LoopCtrl};
use std::time::{Duration, Instant};
use tokio::time::sleep;

pub enum SyncResult {
    Ok(Box<SyncResponse>),
    Failed(usize),
}

#[derive(Debug)]
pub enum SyncLoopMessage {
    LogOut,
    StopSync,
}

pub type SyncLoopChannel = Channel<SyncLoopMessage, SyncLoopMessage>;
pub type SyncLoopChannels = (SyncLoopChannel, Channel<SyncLoopMessage, SyncLoopMessage>);

/// Repeatedly call sync to synchronize the client state with the server.
///
/// # Arguments
///
/// * `sync_settings` - Settings for the sync call. *Note* that those
///   settings will be only used for the first sync call. See the argument
///   docs for [`Client::sync_once`] for more info.
///
/// * `callback` - A callback that will be called every time a
///   response has been fetched from the server. The callback must return a
///   boolean which signalizes if the method should stop syncing. If the
///   callback returns `LoopCtrl::Continue` the sync will continue, if the
///   callback returns `LoopCtrl::Break` the sync will be stopped.
///
pub async fn sync_with_callback<C>(client: &matrix_sdk::Client, callback: impl Fn(SyncResult) -> C)
where
    C: Future<Output = LoopCtrl>,
{
    let mut last_sync_time: Option<Instant> = None;
    let mut sync_settings = SyncSettings::default();

    if let Some(token) = client.sync_token().await {
        sync_settings = sync_settings.token(token);
    }

    let mut attempt = 0;
    loop {
        let response = client.sync_once(sync_settings.clone()).await;

        let result = match response {
            Ok(r) => {
                attempt = 0;
                sync_settings = sync_settings.token(r.next_batch.clone());
                SyncResult::Ok(Box::new(r))
            }
            Err(e) => {
                attempt += 1;
                log::debug!("sync failed ({} time/s): {:#?}", attempt, e);
                sleep(Duration::from_secs(1)).await;
                SyncResult::Failed(attempt)
            }
        };
        if callback(result).await == LoopCtrl::Break {
            return;
        }

        delay_sync(&mut last_sync_time).await
    }
}

async fn delay_sync(last_sync_time: &mut Option<Instant>) {
    let now = Instant::now();

    // If the last sync happened less than a second ago, sleep for a
    // while to not hammer out requests if the server doesn't respect
    // the sync timeout.
    if let Some(t) = last_sync_time {
        if now - *t <= Duration::from_secs(1) {
            sleep(Duration::from_secs(1)).await;
        }
    }

    *last_sync_time = Some(now);
}
///
/// Repeatedly call sync to synchronize the client state with the server.
///
/// It uses `sync_loop_channel` to listen user log out action,
/// or notify user to log out after 3 attempts of sync with failures.
/// If one of those happen, it will break the sync loop.
///  
pub async fn sync(
    client: &matrix_sdk::Client,
    sync_loop_channel: SyncLoopChannel,
) -> impl Future<Output = ()> {
    // client state is held in an `Arc` so the `Client` can be cloned freely.
    let client = client.clone();
    async move {
        sync_with_callback(&client, {
            |sync_result| {
                let sync_loop_channel = sync_loop_channel.clone();
                async move {
                    // user sent the stop sync, so we break the loop
                    if let Ok(SyncLoopMessage::StopSync) = sync_loop_channel.try_recv() {
                        return LoopCtrl::Break;
                    }
                    match sync_result {
                        SyncResult::Ok(_) => {}
                        SyncResult::Failed(attempts) => {
                            if attempts >= 3 {
                                // notify user is not in sync any more and break the loop
                                sync_loop_channel
                                    .send(SyncLoopMessage::LogOut)
                                    .expect("channel to be open");
                                return LoopCtrl::Break;
                            }
                        }
                    }
                    LoopCtrl::Continue
                }
            }
        })
        .await;
    }
}
