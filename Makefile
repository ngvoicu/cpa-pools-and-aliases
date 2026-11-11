ID      := cpa-pools-and-aliases
VERSION := $(shell sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)
EXT := dylib
LIB := libcpa_pools_and_aliases.dylib
else ifneq (,$(findstring MINGW,$(UNAME_S))$(findstring MSYS,$(UNAME_S)))
EXT := dll
LIB := cpa_pools_and_aliases.dll
else
EXT := so
LIB := libcpa_pools_and_aliases.so
endif

# Where `make install` copies the plugin. Must match plugins.dir in your CPA config.
PLUGINS_DIR ?= $(HOME)/.cli-proxy-api/plugins

.PHONY: build test install package clean version

build:
	cargo build --release
	@mkdir -p dist
	cp target/release/$(LIB) dist/$(ID).$(EXT)

test:
	cargo test --release
	node scripts/test-page.mjs

install: build
	@mkdir -p $(PLUGINS_DIR)
	@# copy then rename: never overwrite a library a running CPA has mapped
	cp dist/$(ID).$(EXT) $(PLUGINS_DIR)/.$(ID).$(EXT).tmp
	mv -f $(PLUGINS_DIR)/.$(ID).$(EXT).tmp $(PLUGINS_DIR)/$(ID).$(EXT)
	@echo "installed to $(PLUGINS_DIR)/$(ID).$(EXT)"

# Release asset for one platform, e.g.:
#   make package TARGET=aarch64-apple-darwin GOOS=darwin GOARCH=arm64
package:
	scripts/package.sh $(TARGET) $(GOOS) $(GOARCH)

version:
	@echo $(VERSION)

clean:
	cargo clean
	rm -rf dist build
