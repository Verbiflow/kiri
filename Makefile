.PHONY: check test build bench smoke sdk-check install install-engine

INSTALL_ROOT ?= $(HOME)/.cargo

check:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo test --locked --workspace

test:
	cargo test --locked --workspace

build:
	cargo build --locked --release --workspace

install:
	cargo install --locked --path crates/kiri-cli --root "$(INSTALL_ROOT)"

install-engine:
	cargo install --locked --path crates/kiri-service --bin kiri-engine --root "$(INSTALL_ROOT)"

sdk-check:
	cargo build --locked -p kiri-service --bin kiri-engine
	npm --prefix packages/client ci --ignore-scripts
	npm --prefix packages/client run generate
	npm --prefix packages/client test

bench: build
	./target/release/kiri bench --runs 10

smoke: build
	python3 scripts/verify_tui.py --binary target/release/kiri
	python3 scripts/verify_tui.py --binary target/release/kiri --color always
	python3 scripts/verify_tui.py --binary target/release/kiri --color auto
	python3 scripts/verify_tui.py --binary target/release/kiri --color never
