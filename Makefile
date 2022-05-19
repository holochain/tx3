# tx3 Makefile

.PHONY: all publish test static docs tools tool_rust tool_fmt tool_readme

SHELL = /usr/bin/env sh

all: test

publish:
	@case "$(crate)" in \
		tx3) \
			export MANIFEST="./crates/tx3/Cargo.toml"; \
			;; \
		tx3_pool) \
			export MANIFEST="./crates/tx3_pool/Cargo.toml"; \
			;; \
		*) \
			echo "USAGE: make publish crate=tx3"; \
			echo "USAGE: make publish crate=tx3_pool"; \
			exit 1; \
			;; \
	esac; \
	export VER="v$$(grep version $${MANIFEST} | head -1 | cut -d ' ' -f 3 | cut -d \" -f 2)"; \
	echo "publish $(crate) $${MANIFEST} $${VER}"; \
	git diff --exit-code && \
	cargo publish --manifest-path $${MANIFEST} && \
	git tag -a "$(crate)-$${VER}" -m "$(crate)-$${VER}" && \
	git push --tags;

test: static tools
	RUST_BACKTRACE=1 cargo build --all-features --all-targets
	RUST_BACKTRACE=1 cargo test --all-features -- --test-threads 1

static: docs tools
	cargo fmt -- --check
	cargo clippy

docs: tools
	printf '### The `tx3-relay` executable\n`tx3-relay --help`\n```text\n' > crates/tx3/src/docs/tx3_relay_help.md
	cargo run -- --help >> crates/tx3/src/docs/tx3_relay_help.md
	printf '\n```\n' >> crates/tx3/src/docs/tx3_relay_help.md
	cargo readme -r crates/tx3 -o README.md
	cargo readme -r crates/tx3_pool -o README.md
	printf '\n' >> crates/tx3/README.md
	cat crates/tx3/src/docs/tx3_relay_help.md >> crates/tx3/README.md
	cp crates/tx3/README.md README.md
	@if [ "${CI}x" != "x" ]; then git diff --exit-code; fi

tools: tool_rust tool_fmt tool_clippy tool_readme

tool_rust:
	@if rustup --version >/dev/null 2>&1; then \
		echo "# Makefile # found rustup, setting override stable"; \
		rustup override set stable; \
	else \
		echo "# Makefile # rustup not found, hopefully we're on stable"; \
	fi;

tool_fmt: tool_rust
	@if ! (cargo fmt --version); \
	then \
		if rustup --version >/dev/null 2>&1; then \
			echo "# Makefile # installing rustfmt with rustup"; \
			rustup component add rustfmt; \
		else \
			echo "# Makefile # rustup not found, cannot install rustfmt"; \
			exit 1; \
		fi; \
	else \
		echo "# Makefile # rustfmt ok"; \
	fi;

tool_clippy: tool_rust
	@if ! (cargo clippy --version); \
	then \
		if rustup --version >/dev/null 2>&1; then \
			echo "# Makefile # installing clippy with rustup"; \
			rustup component add clippy; \
		else \
			echo "# Makefile # rustup not found, cannot install clippy"; \
			exit 1; \
		fi; \
	else \
		echo "# Makefile # clippy ok"; \
	fi;

tool_readme: tool_rust
	@if ! (cargo readme --version); \
	then \
		cargo install cargo-readme; \
	else \
		echo "# Makefile # readme ok"; \
	fi;
