FROM rust:alpine AS chef
RUN apk add --no-cache musl-dev openssl-dev
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM scratch AS runtime
COPY --from=builder /app/target/release/tesla-apiscraper-rs /tesla-apiscraper-rs
USER 10000:10001
EXPOSE 4000
ENTRYPOINT ["/tesla-apiscraper-rs"]
