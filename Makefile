.PHONY: build run test lint ci docker-build docker-run build-x86_64-musl cross-setup

APP_NAME ?= tesla-apiscraper-rs
TARGET_X86_64_MUSL ?= x86_64-unknown-linux-musl

build:
	cargo build --release

run:
	cargo run

test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy -- -D warnings

ci: lint test build

docker-build:
	docker build -t $(APP_NAME) .

docker-run:
	docker run --rm $(APP_NAME) --help

cross-setup:
	rustup target add $(TARGET_X86_64_MUSL)
	cargo install cargo-zigbuild

build-x86_64-musl:
	cargo zigbuild --release --locked --target $(TARGET_X86_64_MUSL)

help:
	@perl -nle'print $$& if m{^[a-zA-Z_-]+:.*?#} ' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?# "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'
