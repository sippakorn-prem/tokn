.DEFAULT_GOAL := help

# Signing/notarization secrets for `make release` live in .env (gitignored).
-include .env
export

.PHONY: help install dev build release notary-status check check-frontend check-rust clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

install: ## Install frontend dependencies
	pnpm install

dev: ## Run the app in development mode (tray icon + hot reload)
	pnpm tauri dev

build: ## Build the production app bundle (.app / .dmg)
	pnpm tauri build

release: ## Build a signed + notarized universal DMG (secrets from .env)
	@test -n "$$APPLE_SIGNING_IDENTITY" || { echo "APPLE_SIGNING_IDENTITY not set (see .env)"; exit 1; }
	@test -n "$$APPLE_ID" || { echo "APPLE_ID not set (see .env)"; exit 1; }
	@test -n "$$APPLE_PASSWORD" || { echo "APPLE_PASSWORD not set (see .env)"; exit 1; }
	@case "$$APPLE_PASSWORD" in FILL-ME*) echo "APPLE_PASSWORD still placeholder — put your app-specific password in .env"; exit 1;; esac
	@test -n "$$APPLE_TEAM_ID" || { echo "APPLE_TEAM_ID not set (see .env)"; exit 1; }
	pnpm tauri build --target universal-apple-darwin
	@echo "—— verify ——"
	spctl -a -vv "src-tauri/target/universal-apple-darwin/release/bundle/macos/Tokn.app"
	xcrun stapler validate src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg

notary-status: ## Show recent notarization submissions (secrets from .env)
	@xcrun notarytool history --apple-id "$$APPLE_ID" --password "$$APPLE_PASSWORD" --team-id "$$APPLE_TEAM_ID" | head -12
	@date -u +"          now: %Y-%m-%dT%H:%M:%SZ"

check: check-frontend check-rust ## Type-check frontend and Rust

check-frontend: ## Type-check and build the frontend
	pnpm build

check-rust: ## Type-check the Rust crate
	cd src-tauri && cargo check

clean: ## Remove build artifacts
	rm -rf dist
	cd src-tauri && cargo clean
