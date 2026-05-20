.PHONY: build test bench lint fmt check clean run install

build:
	cargo build --workspace --release

test:
	cargo test --workspace

bench:
	cargo bench --workspace

lint:
	cargo clippy --workspace --all-features -- -D warnings

fmt:
	cargo fmt --all

check: fmt lint test
	@echo "✅ All checks passed"

clean:
	cargo clean

run:
	cargo run --release -p richter-cli -- $(ARGS)

install:
	bash scripts/install.sh
