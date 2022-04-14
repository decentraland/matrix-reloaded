# Matrix Load Testing Tool

## Quick start:

Run this command specifying the Matrix instance to be tested for the `homeserver` argument:

```bash
cargo run --release -- --homeserver HOST
```

### Sample results:

```bash
$ cargo run --release -- --homeserver DEV_SERVER_HOST
   Compiling matrix-load-testing-tool v0.1.0 ($PATH_TO_REPO/matrix-load-testing-tool)
    Finished release [optimized] target(s) in 50.21s
     Running `target/release/matrix-load-testing-tool --homeserver DEV_SERVER_HOST`
Configuration {
    homeserver_url: "DEV_SERVER_HOST",
    output_dir: "output",
    total_steps: 2,
    users_per_step: 10,
    friendship_ratio: 0.1,
    step_duration_in_secs: 30,
    max_users_to_act_per_tick: 100,
    waiting_period: 30,
    retry_request_config: false,
    user_creation_retry_attempts: 5,
}

Running step 1
Amount of users: 10
Amount of friendships: 5
Listing friendships:
- @user_0_1649967588280-user_8_1649967588280:DEV_SERVER_HOST
- @user_1_1649967588280-user_4_1649967588280:DEV_SERVER_HOST
- @user_2_1649967588280-user_5_1649967588280:DEV_SERVER_HOST
- @user_3_1649967588280-user_5_1649967588280:DEV_SERVER_HOST
- @user_7_1649967588280-user_8_1649967588280:DEV_SERVER_HOST

Report generated: output/1649967625850/report_1_1649967625871.yaml

Running step 2
Amount of users: 20
Amount of friendships: 19
Listing friendships:
- @user_0_1649967588280-user_3_1649967588280:DEV_SERVER_HOST
- @user_0_1649967588280-user_8_1649967588280:DEV_SERVER_HOST
- @user_1_1649967588280-user_4_1649967588280:DEV_SERVER_HOST
- @user_2_1649967588280-user_5_1649967588280:DEV_SERVER_HOST
- @user_2_1649967588280-user_9_1649967588280:DEV_SERVER_HOST
- @user_3_1649967588280-user_5_1649967588280:DEV_SERVER_HOST
- @user_5_1649967588280-user_6_1649967588280:DEV_SERVER_HOST
- @user_7_1649967588280-user_8_1649967588280:DEV_SERVER_HOST
- @user_10_1649967625877-user_1_1649967588280:DEV_SERVER_HOST
- @user_11_1649967625877-user_19_1649967625877:DEV_SERVER_HOST
- @user_11_1649967625877-user_7_1649967588280:DEV_SERVER_HOST
- @user_12_1649967625877-user_7_1649967588280:DEV_SERVER_HOST
- @user_14_1649967625877-user_8_1649967588280:DEV_SERVER_HOST
- @user_15_1649967625877-user_1_1649967588280:DEV_SERVER_HOST
- @user_15_1649967625877-user_2_1649967588280:DEV_SERVER_HOST
- @user_16_1649967625877-user_1_1649967588280:DEV_SERVER_HOST
- @user_16_1649967625877-user_2_1649967588280:DEV_SERVER_HOST
- @user_17_1649967625877-user_1_1649967588280:DEV_SERVER_HOST
- @user_17_1649967625877-user_6_1649967588280:DEV_SERVER_HOST

Report generated: output/1649967625850/report_2_1649967665309.yaml
```

For more options and parameters to be configured please see `cargo run -- --help` and the [Config.toml](/Config.toml).



## Contact me:

dservices+matrix-load-testing-tool@decentraland.org
