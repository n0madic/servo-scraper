RELEASE_DIR = target/release

.PHONY: build build-cli build-lib test test-c test-python test-js test-go clean update-servo

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

# Update the Servo submodule to the latest main branch commit
update-servo:
	git -C servo fetch origin
	git -C servo checkout origin/main
	git add servo
	@echo "Servo updated to $$(git -C servo rev-parse --short HEAD). Don't forget to commit."
