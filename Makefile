ID      := cpa-pools-and-aliases
VERSION := $(shell cat VERSION)

# Target platform (Go-style names, used for the release zip name).
HOST_OS   := $(shell uname -s | tr A-Z a-z)
HOST_ARCH := $(shell uname -m | sed -e 's/x86_64/amd64/' -e 's/aarch64/arm64/')
GOOS   ?= $(HOST_OS)
GOARCH ?= $(HOST_ARCH)

ifeq ($(GOOS),darwin)
EXT := dylib
SHARED := -dynamiclib -Wl,-install_name,@rpath/$(ID).dylib
CLANG_ARCH := $(if $(filter amd64,$(GOARCH)),x86_64,arm64)
ARCH_FLAGS ?= -arch $(CLANG_ARCH) -mmacosx-version-min=11.0
else ifeq ($(GOOS),windows)
EXT := dll
SHARED := -shared -static-libgcc
ARCH_FLAGS ?=
else
EXT := so
SHARED := -shared
ARCH_FLAGS ?=
endif

CC      ?= cc
CFLAGS  ?= -O2
CFLAGS  += -std=c11 -Wall -Wextra -fPIC -fvisibility=hidden -DPLUGIN_VERSION=\"$(VERSION)\" -Ibuild
PYTHON  ?= python3

LIB := dist/$(ID).$(EXT)
ZIP := dist/$(ID)_$(VERSION)_$(GOOS)_$(GOARCH).zip

# Where `make install` copies the plugin. Must match plugins.dir in your CPA config.
PLUGINS_DIR ?= $(HOME)/.cli-proxy-api/plugins

.PHONY: all build package install clean version

all: build

build: $(LIB)

build/page.h: web/index.html scripts/embed.py
	@mkdir -p build
	$(PYTHON) scripts/embed.py web/index.html $@

$(LIB): src/plugin.c build/page.h VERSION
	@mkdir -p dist
	$(CC) $(CFLAGS) $(ARCH_FLAGS) $(SHARED) -o $@ src/plugin.c

# Release asset: zip with the library at the zip root, named <id>.<ext>.
package: $(LIB)
	@rm -f $(ZIP)
	cd dist && zip -q -X $(notdir $(ZIP)) $(notdir $(LIB))
	@echo "packaged $(ZIP)"

install: $(LIB)
	@mkdir -p $(PLUGINS_DIR)
	cp $(LIB) $(PLUGINS_DIR)/$(ID).$(EXT)
	@echo "installed to $(PLUGINS_DIR)/$(ID).$(EXT)"

version:
	@echo $(VERSION)

clean:
	rm -rf build dist
