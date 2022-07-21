# Matrix Reloaded

A tool that can be used to simulate Matrix users behavior for some period of time.

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

#### Notes
- You might need to install XCode first from the App Store (not the Command Line Tools)
- If Command Line Tools where already present in the system you might need to first 1) make sure XCode is in the `/Applications` directory and not the user apps one and 2) point xcode-select to the actual XCode app by using: `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`

## Quick start

1. Usage:

```
cargo run -- [OPTIONS] --homeserver <HOMESERVER> [OUTPUT]

OPTIONS:
    -d, --duration <DURATION>                Tick duration in seconds
    -h, --homeserver <HOMESERVER>            Homeserver to use during the simulation
    -m, --max-users <MAX_USERS>              Max number of users for current simulation
    -t, --ticks <TICKS>                      Number of times to tick during the simulation
    -u, --users-per-tick <USERS_PER_TICK>    Number of users to act during the simulation
```

### Sample results

After running the test, a directory with the current run will be created in the output directory (`output/{timestamp}` by default) with a file report for each step (e.g. `output/1650978209761/report_1_1650978284466.yaml`) with the following data:

```yaml
---
requests_average_time:
  login: 2434
  create_room: 1472
  send_message: 972
total_requests:
  create_room: 16
  send_message: 264
  login: 201
http_errors_per_request:
  create_room_400: 8
message_delivery_average_time: 2508
messages_sent: 151
messages_not_sent: 0
real_time_messages: 113
```

For more options and parameters to be configured please see `cargo run -- --help` and the [configuration.toml](/configuration.toml).

## Contact me

dservices+matrix-load-testing-tool@decentraland.org
