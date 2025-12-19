
check:
	@echo "Rust checks disabled; use Python tooling in python/"

test:
	@echo "Rust tests disabled; use pytest in python/"

fmt:
	@echo "Rust formatting disabled; use ruff/black in python/"

docker:
	docker build .

addon:
	docker run \
		--rm \
		--privileged \
		-v /var/run/docker.sock:/var/run/docker.sock \
		-v ./addon:/data \
			ghcr.io/home-assistant/amd64-builder:latest \
			--all \
			--test \
			--target /data

# This will start hass on http://localhost:7123
container:
	npm install @devcontainers/cli
	npx @devcontainers/cli up --workspace-folder .
	npx @devcontainers/cli exec --workspace-folder . supervisor_run

.PHONY: addon fmt check hass
