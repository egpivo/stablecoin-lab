.PHONY: build fetch pin verify test lint parity scatter

build:
	cargo run --release -- build

fetch:
	cargo run --release -- fetch

pin:
	cargo run --release -- pin

verify:
	cargo run --release -- verify

test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

# Compare data/out/map_data.json with a prototype JSON: make parity PROTO=path/to/map_data.json
parity:
	python3 python/parity_check.py data/out/map_data.json $(PROTO)

scatter:
	python3 python/plot_scatter.py data/out/purchasing_power.csv --month 2026-09 --out data/out/scatter_usd.png
