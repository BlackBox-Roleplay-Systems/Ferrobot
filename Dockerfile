# Build
FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# Run
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 ferrobot
COPY --from=build /src/target/release/ferrobot /usr/local/bin/ferrobot
USER ferrobot
ENTRYPOINT ["/usr/local/bin/ferrobot"]
