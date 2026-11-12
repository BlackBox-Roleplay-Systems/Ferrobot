# Ferrobot

A Second Life bot written in Rust, in the spirit of [SecondBot](https://github.com/Madpeterz/SecondBot) (.NET), run as a Docker container.

## Build and run

```
cargo build --release
docker build -t ferrobot .
docker run --rm ferrobot
```
