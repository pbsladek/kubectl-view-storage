.PHONY: check test contract coverage plugin-smoke release-verify release-smoke preflight inventory-create inventory-verify inventory-delete csi-create csi-verify csi-delete nfs-create nfs-verify nfs-delete storage-e2e

RELEASE_DIRECTORY ?= artifacts/release

check:
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

contract:
	uv run scripts/validate-contract.py

coverage:
	sh scripts/coverage.sh

plugin-smoke:
	python3 scripts/check-plugin.py

release-verify:
	python3 scripts/package-release.py verify --directory "$(RELEASE_DIRECTORY)"

release-smoke:
	python3 scripts/package-release.py smoke --directory "$(RELEASE_DIRECTORY)"

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
