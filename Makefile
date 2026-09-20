.PHONY: build run test lint ci docker-build docker-run build-x86_64-musl cross-setup

APP_NAME ?= tesla-apiscraper-rs
TARGET_X86_64_MUSL ?= x86_64-unknown-linux-musl
ZIGBUILD_VERSION ?= 0.23.4

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
	@command -v zig >/dev/null || (echo "error: zig not found — install it first: brew install zig (macOS) or https://ziglang.org/download/" && exit 1)
	cargo install cargo-zigbuild --version $(ZIGBUILD_VERSION) --locked

build-x86_64-musl:
	cargo zigbuild --release --locked --target $(TARGET_X86_64_MUSL)

help:
	@perl -nle'print $$& if m{^[a-zA-Z_-]+:.*?#} ' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?# "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'
