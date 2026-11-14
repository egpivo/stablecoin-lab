.PHONY: build fetch pin verify test fmt lint coverage pre-commit parity scatter

build:
	cargo run --release -- build

fetch:
	cargo run --release -- fetch

pin:
	cargo run --release -- pin

verify:
	cargo run --release -- verify

test:
	cargo test --all-targets

fmt:
	cargo fmt --all

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings

coverage:
	cargo llvm-cov --workspace --all-features --ignore-filename-regex '(main|fetch)\.rs'

pre-commit:
	pre-commit run --all-files

# Compare data/out/map_data.json with a prototype JSON: make parity PROTO=path/to/map_data.json
parity:
	python3 python/parity_check.py data/out/map_data.json $(PROTO)

scatter:
	python3 python/plot_scatter.py data/out/purchasing_power.csv --month 2026-09 --out data/out/scatter_usd.png
