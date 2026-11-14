.PHONY: build fetch pin verify test fmt lint pre-commit parity scatter

build:
	cargo run --release -p stablecoin-value -- build

fetch:
	cargo run --release -p stablecoin-value -- fetch

pin:
	cargo run --release -p stablecoin-value -- pin

verify:
	cargo run --release -p stablecoin-value -- verify

test:
	cargo test -p stablecoin-value --all-targets

fmt:
	cargo fmt --all

lint:
	cargo fmt --all -- --check
	cargo clippy -p stablecoin-value --all-targets --all-features -- -D warnings

pre-commit:
	pre-commit run --all-files

# Compare data/out/map_data.json with a prototype JSON: make parity PROTO=path/to/map_data.json
parity:
	python3 python/parity_check.py data/out/map_data.json $(PROTO)

scatter:
	python3 python/plot_scatter.py data/out/purchasing_power.csv --month 2026-09 --out data/out/scatter_usd.png
