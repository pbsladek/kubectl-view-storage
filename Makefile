.PHONY: verify check completions complexity test contract coverage plugin-smoke release-verify release-smoke preflight inventory-create inventory-verify inventory-delete csi-create csi-verify csi-delete nfs-create nfs-verify nfs-delete storage-e2e

RELEASE_DIRECTORY ?= artifacts/release$(if $(VERSION),-$(VERSION))
export RELEASE_DIRECTORY
export RELEASE_VERSION = $(VERSION)
export RELEASE_RUN_ID = $(RUN_ID)
export RELEASE_DRAFT = $(DRAFT)
.DEFAULT_GOAL := check

.PHONY: release-help release-prepare release release-build release-publish

release-help:
	python3 scripts/release.py help

release-prepare:
	python3 scripts/release.py prepare

release: release-build

release-build:
	python3 scripts/release.py build

release-publish:
	python3 scripts/release.py publish

verify:
	python3 scripts/verify.py

completions:
	cargo run --locked --example completions

check:
	python3 scripts/test-release.py
	python3 scripts/test-verify.py
	uv run scripts/test-complexity.py
	uv run scripts/check-complexity.py
	python3 scripts/test-ci-scope.py
	python3 scripts/test-toolchain-check.py
	python3 scripts/test-ci-timings.py
	python3 scripts/check-toolchain.py
	python3 scripts/check-dependencies.py
	python3 scripts/test-dependency-check.py
	python3 scripts/test-dev-environment.py
	python3 scripts/test-csi-environment.py
	python3 scripts/test-dev-calculations.py
	python3 scripts/test-storage-e2e.py
	python3 scripts/test-release-packaging.py
	python3 scripts/test-smoke-http.py
	cargo fmt --all --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo test --locked --all-targets
	python3 scripts/test-credential-terminal.py
	python3 scripts/test-coverage-check.py

test:
	cargo test --locked --all-targets

complexity:
	uv run scripts/check-complexity.py

contract:
	uv run scripts/validate-contract.py

coverage:
	sh scripts/coverage.sh

plugin-smoke:
	python3 scripts/check-plugin.py

release-verify:
	python3 scripts/package-release.py verify --directory "$$RELEASE_DIRECTORY"

release-smoke:
	python3 scripts/package-release.py smoke --directory "$$RELEASE_DIRECTORY"

preflight:
	python3 scripts/dev-cluster.py preflight inventory

inventory-create:
	python3 scripts/dev-cluster.py create inventory

inventory-verify:
	python3 scripts/dev-cluster.py verify inventory

inventory-delete:
	python3 scripts/dev-cluster.py delete inventory

csi-create:
	python3 scripts/dev-cluster.py create csi

csi-verify:
	python3 scripts/dev-cluster.py verify csi

csi-delete:
	python3 scripts/dev-cluster.py delete csi

nfs-create:
	python3 scripts/dev-cluster.py create nfs

nfs-verify:
	python3 scripts/dev-cluster.py verify nfs

nfs-delete:
	python3 scripts/dev-cluster.py delete nfs

storage-e2e:
	python3 scripts/e2e-storage.py
