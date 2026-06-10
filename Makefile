.DEFAULT_GOAL := help

.PHONY: help install dev build check check-frontend check-rust clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

install: ## Install frontend dependencies
	pnpm install

dev: ## Run the app in development mode (tray icon + hot reload)
	pnpm tauri dev

build: ## Build the production app bundle (.app / .dmg)
	pnpm tauri build

check: check-frontend check-rust ## Type-check frontend and Rust

check-frontend: ## Type-check and build the frontend
	pnpm build

check-rust: ## Type-check the Rust crate
	cd src-tauri && cargo check

clean: ## Remove build artifacts
	rm -rf dist
	cd src-tauri && cargo clean
