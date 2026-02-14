RELEASE_DIR = target/release
STATIC_LIB  = $(RELEASE_DIR)/libservo_scraper.a

# macOS frameworks required by Servo
FRAMEWORKS = -framework AppKit \
             -framework CoreFoundation \
             -framework CoreGraphics \
             -framework CoreText \
             -framework IOSurface \
             -framework Metal \
             -framework OpenGL \
             -framework QuartzCore \
             -framework Security \
             -framework SystemConfiguration

# -no_fixup_chains: required because SpiderMonkey C++ objects
# contain unaligned pointers that Apple's new linker rejects
LDFLAGS = $(STATIC_LIB) $(FRAMEWORKS) -lc++ -lresolv -lz \
          -Wl,-no_fixup_chains

.PHONY: build build-cli build-lib test-c test-python test-js clean update-servo

# Build everything (CLI binary + shared/static libraries)
build:
	cargo build --release

# Build only the CLI binary
build-cli:
	cargo build --release --bin servo-scraper

# Build only the library (rlib + cdylib + staticlib)
build-lib:
	cargo build --release --lib

# Build and link the C example against the static library
test-c: build-lib
	cc -o $(RELEASE_DIR)/test_scraper \
		examples/c/test_scraper.c \
		-Iexamples/c \
		$(LDFLAGS)
	@echo "Built: $(RELEASE_DIR)/test_scraper"

# Verify the Python example can load the shared library
test-python: build-lib
	python3 -c "\
		import ctypes, sys; \
		lib = ctypes.CDLL('$(RELEASE_DIR)/libservo_scraper.dylib'); \
		assert lib.scraper_new, 'scraper_new not found'; \
		assert lib.scraper_free, 'scraper_free not found'; \
		assert lib.scraper_screenshot, 'scraper_screenshot not found'; \
		assert lib.scraper_html, 'scraper_html not found'; \
		print('Python: loaded libservo_scraper.dylib, all FFI symbols found')"

# Install JS dependencies and verify the library can be loaded
test-js: build-lib
	cd examples/js && npm install --silent
	NODE_PATH=examples/js/node_modules node -e "\
		const koffi = require('koffi'); \
		const lib = koffi.load('$(RELEASE_DIR)/libservo_scraper.dylib'); \
		const f = lib.func('void *scraper_new(uint32_t, uint32_t, uint64_t, double, int)'); \
		console.log('Node.js: loaded libservo_scraper.dylib via koffi, FFI binding OK');"

clean:
	cargo clean

update-servo:
	git -C servo fetch origin
	git -C servo checkout origin/main
	git add servo
	@echo "Servo updated to $$(git -C servo rev-parse --short HEAD). Don't forget to commit."
