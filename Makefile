# tx3 Makefile

.PHONY: all test docs tools tool_rust tool_fmt tool_readme

SHELL = /usr/bin/env sh

all: test

test: docs tools
	cargo fmt -- --check
	cargo clippy
	RUST_BACKTRACE=1 cargo test --all-features --all-targets
	@if [ "${CI}x" != "x" ]; then git diff --exit-code; fi

docs: tools
	printf '### The `tx3-relay` executable\n`tx3-relay --help`\n```no-compile\n' > src/docs/tx3_relay_help.md
	cargo run -- --help >> src/docs/tx3_relay_help.md
	printf '\n```\n' >> src/docs/tx3_relay_help.md
	cargo readme -o README.md
	printf '\n' >> README.md
	cat src/docs/tx3_relay_help.md >> README.md

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
