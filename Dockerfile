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

# SolidJS SPA (Vite build output goes to /web/dist).
FROM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM scratch AS runtime
COPY --from=builder /app/target/release/tesla-apiscraper-rs /tesla-apiscraper-rs
COPY --from=web /web/dist /web/dist
ENV WEB_DIST_DIR=/web/dist
USER 10000:10001
EXPOSE 4000
ENTRYPOINT ["/tesla-apiscraper-rs"]
