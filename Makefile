
.PHONY:
all: build deploy

.PHONY: build
build:
	cargo build --release

.PHONY: deploy
deploy: build
	mkdir -p lua
	mv target/release/libtwitch.so lua/twitch.so
