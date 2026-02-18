RELEASE_DIR = target/release
DIST_DIR = dist
VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# Platform-specific library extensions
DYLIB_EXT_MACOS = dylib
DYLIB_EXT_LINUX = so

# Target triples
TARGET_MACOS_ARM64 = aarch64-apple-darwin
TARGET_MACOS_X86_64 = x86_64-apple-darwin
TARGET_LINUX_X86_64 = x86_64-unknown-linux-gnu

.PHONY: build build-cli build-lib test test-c test-python test-js test-go clean update-servo \
	release-macos-arm64 release-macos-x86_64 release-linux-x86_64 release-all release

# Build everything (CLI binary + shared/static libraries)
build:
	cargo build --release

# Run integration tests (single-threaded — PageEngine is !Send+!Sync)
test:
	cargo test -- --test-threads=1

# Build only the CLI binary
build-cli:
	cargo build --release --bin servo-scraper

# Build only the library (rlib + cdylib + staticlib)
build-lib:
	cargo build --release --lib

# Build the C example against the shared library
test-c: build-lib
	cc -o $(RELEASE_DIR)/test_scraper \
		examples/c/test_scraper.c \
		-Iexamples/c \
		-L$(RELEASE_DIR) -lservo_scraper
	@echo "Built: $(RELEASE_DIR)/test_scraper"

# Verify the Python example can load the shared library
test-python: build-lib
	python3 -c "\
		import ctypes, sys; \
		lib = ctypes.CDLL('$(RELEASE_DIR)/libservo_scraper.dylib'); \
		assert lib.page_new, 'page_new not found'; \
		assert lib.page_free, 'page_free not found'; \
		assert lib.page_open, 'page_open not found'; \
		assert lib.page_screenshot, 'page_screenshot not found'; \
		assert lib.page_html, 'page_html not found'; \
		assert lib.page_evaluate, 'page_evaluate not found'; \
		print('Python: loaded libservo_scraper.dylib, all FFI symbols found')"

# Install JS dependencies and verify the library can be loaded
test-js: build-lib
	cd examples/js && npm install --silent
	NODE_PATH=examples/js/node_modules node -e "\
		const koffi = require('koffi'); \
		const lib = koffi.load('$(RELEASE_DIR)/libservo_scraper.dylib'); \
		const f = lib.func('void *page_new(uint32_t, uint32_t, uint64_t, double, int)'); \
		console.log('Node.js: loaded libservo_scraper.dylib via koffi, FFI binding OK');"

# Build the Go example against the shared library
test-go: build-lib
	CGO_ENABLED=1 go build -o $(RELEASE_DIR)/go_scraper ./examples/go/scraper.go
	@echo "Built: $(RELEASE_DIR)/go_scraper"

# Clean build artifacts
clean:
	cargo clean
	rm -rf $(DIST_DIR)

# Update the Servo submodule to the latest main branch commit
update-servo:
	git -C servo fetch origin
	git -C servo checkout origin/main
	git add servo
	@echo "Servo updated to $$(git -C servo rev-parse --short HEAD). Don't forget to commit."

# ---------------------------------------------------------------------------
# Release packaging
# ---------------------------------------------------------------------------
# Prerequisites (one-time setup):
#   rustup target add x86_64-apple-darwin
#   cargo install cross --git https://github.com/cross-rs/cross
#   Docker must be running (for Linux builds)
#   brew install gh && gh auth login

define package_release
	@echo "==> Packaging $(1)..."
	mkdir -p $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)
	cp $(2)/servo-scraper                     $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)/
	cp $(2)/libservo_scraper.$(3)             $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)/
	cp $(2)/libservo_scraper.a                $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)/
	cp examples/c/servo_scraper.h             $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)/
	cp README.md                              $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)/
	tar -czf $(DIST_DIR)/servo-scraper-$(VERSION)-$(1).tar.gz \
		-C $(DIST_DIR) servo-scraper-$(VERSION)-$(1)
	rm -rf $(DIST_DIR)/servo-scraper-$(VERSION)-$(1)
	@echo "==> Created $(DIST_DIR)/servo-scraper-$(VERSION)-$(1).tar.gz"
endef

# macOS ARM64 (native build on Apple Silicon)
release-macos-arm64:
	cargo build --release
	$(call package_release,macos-arm64,target/release,$(DYLIB_EXT_MACOS))

# macOS x86_64 (cross-compile via Apple universal SDK)
release-macos-x86_64:
	cargo build --release --target $(TARGET_MACOS_X86_64)
	$(call package_release,macos-x86_64,target/$(TARGET_MACOS_X86_64)/release,$(DYLIB_EXT_MACOS))

# Linux x86_64 (cross-compile via Docker using `cross`)
release-linux-x86_64:
	cross build --release --target $(TARGET_LINUX_X86_64)
	$(call package_release,linux-x86_64,target/$(TARGET_LINUX_X86_64)/release,$(DYLIB_EXT_LINUX))

# Build all platforms
release-all: release-macos-arm64 release-macos-x86_64 release-linux-x86_64

# Full release: build all platforms, tag, push, and create GitHub release
release: release-all
	@if [ -z "$(VERSION)" ]; then echo "ERROR: VERSION not set"; exit 1; fi
	@echo "==> Creating release v$(VERSION)..."
	git tag -a "v$(VERSION)" -m "Release v$(VERSION)" 2>/dev/null || \
		echo "    Tag v$(VERSION) already exists, skipping"
	git push origin "v$(VERSION)"
	gh release create "v$(VERSION)" $(DIST_DIR)/*.tar.gz \
		--title "v$(VERSION)" \
		--generate-notes
	@echo "==> Release v$(VERSION) published!"
