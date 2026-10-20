PREFIX ?= $(HOME)/.local

.PHONY: build install run test

build:
	cargo build --release

install: build
	install -Dm755 target/release/mascarpone $(PREFIX)/bin/mascarpone

run:
	cargo run

test:
	cargo test
