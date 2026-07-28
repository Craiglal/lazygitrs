# lazygitrs — build & release commands.
#
# The justfile remains the historical task runner; this Makefile covers the
# build and release paths so the project is fully usable with nothing but
# `make` and a Rust toolchain. Run `make` (or `make help`) for the target list.
#
# Release flow, for orientation:
#   make preflight   → clean tree + fmt + clippy + test + optimized build
#   make release     → tag_and_release.sh bumps Cargo.toml/npm, regenerates
#                      CHANGELOG.md, commits, tags v<x.y.z>, pushes
#   GitHub Actions   → cargo-dist builds per-target artifacts, creates the
#                      GitHub Release, publishes the Homebrew formula
#   make publish-crate / publish-npm → still manual, opt-in (CONFIRM=1)

SHELL := /usr/bin/env bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := help

CARGO ?= cargo
BIN := lazygitrs
NAME := $(shell sed -n 's/^name *= *"\([^"]*\)".*/\1/p' Cargo.toml)
VERSION := $(shell sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml)
NPM_VERSION := $(shell sed -n 's/.*"version":[[:space:]]*"\([^"]*\)".*/\1/p' npm/package.json)
DEBUG_BIN := target/debug/$(BIN)
RELEASE_BIN := target/release/$(BIN)

# Extra flags forwarded to cargo, e.g. `make test CARGO_ARGS="--nocapture"`.
CARGO_ARGS ?=

# Abort with an install hint when a release-only tool is missing. $(1)=binary,
# $(2)=how to get it. Used by the git-cliff / dist / bun targets below.
define require_tool
@command -v $(1) >/dev/null 2>&1 || { \
	printf '\033[31m✗ `%s` is required by `make %s` but was not found.\033[0m\n' '$(1)' '$@' >&2; \
	printf '  install with: %s\n' '$(2)' >&2; \
	exit 1; \
}
endef

define require_clean_tree
@test -z "$$(git status --porcelain)" || { \
	printf '\033[31m✗ Working tree is dirty; commit or stash before `make %s`.\033[0m\n' '$@' >&2; \
	git status --short >&2; \
	exit 1; \
}
endef

##@ Build

.PHONY: build
build: ## Debug build (fast compile, unoptimized binary)
	$(CARGO) build $(CARGO_ARGS)

.PHONY: build-release
build-release: ## Optimized build (lto + strip, per [profile.release])
	$(CARGO) build --release $(CARGO_ARGS)

.PHONY: check
check: ## Type-check without producing a binary
	$(CARGO) check --all-targets $(CARGO_ARGS)

.PHONY: install
install: ## Install lazygitrs into ~/.cargo/bin from this checkout
	$(CARGO) install --path . --force

.PHONY: uninstall
uninstall: ## Remove the cargo-installed lazygitrs
	$(CARGO) uninstall $(NAME)

.PHONY: clean
clean: ## Remove target/
	$(CARGO) clean

##@ Run

.PHONY: run
run: ## Build and run the debug binary via cargo
	$(CARGO) run $(CARGO_ARGS)

.PHONY: preview
preview: $(DEBUG_BIN) ## Run the debug binary directly (justfile: preview)
	./$(DEBUG_BIN)

.PHONY: rpreview
rpreview: $(RELEASE_BIN) ## Run the release binary directly (justfile: rpreview)
	./$(RELEASE_BIN)

$(DEBUG_BIN):
	$(MAKE) build

$(RELEASE_BIN):
	$(MAKE) build-release

##@ Quality

.PHONY: test
test: ## Run the test suite
	$(CARGO) test $(CARGO_ARGS)

.PHONY: fmt
fmt: ## Format the source tree in place
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Fail if the source tree is not formatted
	$(CARGO) fmt --all -- --check

# Advisory on purpose: the tree currently carries ~190 clippy hits (mostly
# edition-2024 style lints such as collapsible_if), so `-D warnings` would make
# every release gate unpassable. Use clippy-strict when chipping away at them.
.PHONY: clippy
clippy: ## Lint with clippy (reports warnings, does not fail)
	$(CARGO) clippy --all-targets

.PHONY: clippy-strict
clippy-strict: ## Lint with clippy treating warnings as errors (currently fails)
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: lint
lint: fmt-check clippy ## fmt-check + advisory clippy

.PHONY: ci
ci: lint test build-release ## Everything CI would gate a release on

##@ Release

.PHONY: version
version: ## Print the versions recorded in Cargo.toml and npm/package.json
	@printf '%-18s %s\n%-18s %s\n' 'Cargo.toml' '$(VERSION)' 'npm/package.json' '$(NPM_VERSION)'

.PHONY: version-check
version-check: ## Fail if Cargo.toml and npm/package.json versions disagree
	@test '$(VERSION)' = '$(NPM_VERSION)' || { \
		printf '\033[31m✗ Version mismatch: Cargo.toml=%s npm/package.json=%s\033[0m\n' '$(VERSION)' '$(NPM_VERSION)' >&2; \
		exit 1; \
	}
	@printf '\033[32m✓ %s v%s\033[0m\n' '$(NAME)' '$(VERSION)'

.PHONY: sync-readme
sync-readme: ## Copy README.md into npm/ (required after any README edit)
	cp README.md npm/README.md

.PHONY: readme-check
readme-check: ## Fail if npm/README.md has drifted from README.md
	@diff -q README.md npm/README.md >/dev/null || { \
		printf '\033[31m✗ npm/README.md is stale; run `make sync-readme`.\033[0m\n' >&2; \
		exit 1; \
	}

.PHONY: changelog
changelog: ## Preview the unreleased CHANGELOG section (does not write)
	$(call require_tool,git-cliff,cargo install git-cliff)
	git cliff --unreleased

.PHONY: clean-tree-check
clean-tree-check: ## Fail if the working tree has uncommitted changes
	$(call require_clean_tree)

# Prerequisite order matters: the tree check is cheap and runs first so a dirty
# checkout fails before the optimized build burns time.
.PHONY: preflight
preflight: clean-tree-check version-check readme-check ci ## Pre-release gate: tree, versions, README, lint, test, build
	@printf '\033[32m✓ preflight passed for %s v%s\033[0m\n' '$(NAME)' '$(VERSION)'

.PHONY: release
release: preflight ## Bump, changelog, commit, tag and push (interactive; triggers CI release)
	$(call require_tool,git-cliff,cargo install git-cliff)
	sh tag_and_release.sh

.PHONY: release-fast
release-fast: ## Same as `release` but skips the preflight gate
	$(call require_tool,git-cliff,cargo install git-cliff)
	sh tag_and_release.sh

.PHONY: dist-plan
dist-plan: ## Show the cargo-dist release plan without building
	$(call require_tool,dist,cargo install cargo-dist)
	dist plan

.PHONY: dist-build
dist-build: ## Build this host's release artifacts locally via cargo-dist
	$(call require_tool,dist,cargo install cargo-dist)
	dist build

.PHONY: publish-crate
publish-crate: ## Publish to crates.io — manual, requires CONFIRM=1
	@test '$(CONFIRM)' = '1' || { \
		printf '\033[33m! This publishes %s v%s to crates.io and cannot be undone.\033[0m\n' '$(NAME)' '$(VERSION)' >&2; \
		printf '  Re-run as: make publish-crate CONFIRM=1\n' >&2; \
		exit 1; \
	}
	$(call require_clean_tree)
	$(CARGO) publish

.PHONY: publish-npm
publish-npm: sync-readme ## Publish the npm wrapper — manual, requires CONFIRM=1
	@test '$(CONFIRM)' = '1' || { \
		printf '\033[33m! This publishes %s v%s to npm and cannot be undone.\033[0m\n' '$(NAME)' '$(NPM_VERSION)' >&2; \
		printf '  Re-run as: make publish-npm CONFIRM=1\n' >&2; \
		exit 1; \
	}
	npm publish ./npm

##@ Nix

.PHONY: nix-build
nix-build: ## Build the flake's default package
	nix build .#lazygitrs

.PHONY: nix-run
nix-run: ## Run the flake's default app
	nix run .

##@ Generators

.PHONY: gen-benchmarks
gen-benchmarks: ## Regenerate README benchmarks, then sync to npm/
	$(call require_tool,bun,https://bun.sh)
	bun scripts/gen-benchmarks.ts
	$(MAKE) sync-readme

.PHONY: gen-themes
gen-themes: ## Regenerate src/generated_themes
	$(call require_tool,bun,https://bun.sh)
	bun scripts/gen-themes.ts

.PHONY: ref-pull
ref-pull: ## Update the _tmp_* reference checkouts
	$(call require_tool,bun,https://bun.sh)
	bun scripts/fetch-references.ts pull

.PHONY: ref-clone
ref-clone: ## Clone the _tmp_* reference checkouts
	$(call require_tool,bun,https://bun.sh)
	bun scripts/fetch-references.ts clone

##@ Help

.PHONY: help
help: ## Show this help
	@awk 'BEGIN { FS = ":.*##"; \
		printf "\n  \033[1m%s\033[0m v%s — make targets\n", "$(NAME)", "$(VERSION)" } \
		/^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } \
		/^[a-zA-Z0-9_-]+:.*##/ { printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo
