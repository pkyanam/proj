.PHONY: build release install clean test

# Build debug version
build:
	cargo build

# Build release version
release:
	cargo build --release

# Install to ~/.local/bin
install: release
	mkdir -p ~/.local/bin
	cp target/release/proj ~/.local/bin/
	cp target/release/proj-daemon ~/.local/bin/
	@echo ""
	@echo "Installed to ~/.local/bin/"
	@echo "Make sure ~/.local/bin is in your PATH"

# Run tests
test:
	cargo test

# Clean build artifacts
clean:
	cargo clean
	rm -f ~/.proj/daemon.sock ~/.proj/daemon.pid

# Stop the daemon
stop-daemon:
	pkill -f proj-daemon || true
	rm -f ~/.proj/daemon.sock ~/.proj/daemon.pid

# Development: rebuild and restart daemon
dev: build stop-daemon
	./target/debug/proj daemon -f
