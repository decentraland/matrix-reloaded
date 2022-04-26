# Matrix Load Testing Tool

## Setup

Before running the commands below remember to install the linker used by your OS:

### On Windows:

```
cargo install -f cargo-binutils
rustup component add llvm-tools-preview
```

### On Linux:

- Ubuntu

  ```bash
  sudo apt-get install lld clang
  ```

- Arch
  ```bash
  sudo pacman -S lld clang
  ```

### On MacOS:

```bash
brew install michaeleisel/zld/zld
```

## Quick start

1. First of all, copy the [`Config.toml.example`](Config.toml.example) file in the root directory of the project to `Config.toml`, this file will be ignored in the git repository.

1. Make sure users are available for the server you will be testing by checking the [`users.json`](users.json) file, or create them by running:

   ```
   cargo run --release -- --create --amount NUMBER_OF_USERS_TO_CREATE --homeserver HOST
   ```

1. Run this command specifying the Matrix instance to be tested for the `homeserver` argument:

   ```bash
   cargo run --release -- --run --homeserver HOST
   ```

### Sample results

After running the test, a directory with the current run will be created in the output directory (`output/{timestamp}` by default) with a file report for each step (e.g. `output/1650978209761/report_1_1650978284466.yaml`) with the following data:

```yaml
---
homeserver: HOMESERVER_URL
step: 1
step_users: 20
step_friendships: 19
report:
  requests_average_time:
    create_room: 4128
    login: 2160
    send_message: 471
    join_room: 331
  http_errors_per_request: {}
  message_delivery_average_time: 54
  messages_sent: 96
  lost_messages: 0
```

For more options and parameters to be configured please see `cargo run -- --help` and the [Config.toml.example](/Config.toml.example).

## Contact me

dservices+matrix-load-testing-tool@decentraland.org
