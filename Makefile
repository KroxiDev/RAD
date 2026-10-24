export CARGO_HOME ?= $(CURDIR)/target/cargo-home
export CARGO_TARGET_DIR ?= $(CURDIR)/target
export CARGO_BUILD_JOBS ?= $(shell nproc | awk '{print ($$1 < 8 ? $$1 : 8)}')
export CARGO_INCREMENTAL ?= 0

.PHONY: up up-full

up:
	cargo build --offline --locked --release --workspace --bins

up-full: up
	"$(CARGO_TARGET_DIR)/release/rad-editor"
