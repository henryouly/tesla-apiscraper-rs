.PHONY: build run test lint ci docker-build docker-run

APP_NAME ?= teslamate-rs

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

help:
	@perl -nle'print $$& if m{^[a-zA-Z_-]+:.*?#} ' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?# "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'
