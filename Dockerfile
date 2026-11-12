# Build
FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p ferrobot

# Run
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 ferrobot
COPY --from=build /src/target/release/ferrobot /usr/local/bin/ferrobot
USER ferrobot
# healthy while online, or offline for less than 10 minutes (a relog); see services/health.rs
HEALTHCHECK --interval=30s --timeout=5s --start-period=2m CMD ["/usr/local/bin/ferrobot", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/ferrobot"]
