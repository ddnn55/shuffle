# shuffle

`shuffle` is a terminal MP3 player for macOS.

It scans a folder for `.mp3` files, shuffles playback, and provides a simple TUI with keyboard and mouse controls.

The iOS app in this repository is experimental and is not part of the Homebrew distribution.

## Features

- Shuffle playback across a folder of MP3s
- Track metadata display from ID3 tags when available
- Previous/next controls
- Play/pause support
- Resume previous track and position on relaunch
- macOS media remote integration

## Requirements

- macOS
- Apple developer tools with `swiftc` available

The project uses Rust for the main binary and compiles a small macOS Swift helper during the build.

## Run From Source

```bash
cargo run --release -- /path/to/music-folder
```

If you omit the path, `shuffle` scans the current directory for MP3 files.

## Controls

- `space`: play/pause
- `left` or `h`: previous track
- `right` or `l`: next track
- `q` or `esc`: quit

## Local Homebrew Install

Public install from the tap:

```bash
brew install ddnn55/shuffle/shuffle
```

Or tap first and then install:

```bash
brew tap ddnn55/shuffle
brew install ddnn55/shuffle/shuffle
```

For local development against this checkout, use a symlinked local tap:

```bash
mkdir -p "$(brew --repository)/Library/Taps/local"
ln -s "$PWD" "$(brew --repository)/Library/Taps/local/homebrew-shuffle"
brew install --build-from-source local/shuffle/shuffle
```

Then run:

```bash
shuffle /path/to/music-folder
```

Useful commands:

```bash
brew reinstall ddnn55/shuffle/shuffle
brew uninstall shuffle
```

The Homebrew formula installs only the `shuffle` command-line tool.

## State

Playback state is stored at:

```text
~/.shuffle/state
```
