# Regenerate fixture .dll files from their .cs sources. Mirrors the
# pattern used by crates/dll-diff/tests/fixtures — `make` rebuilds
# everything that's stale, `make CASE=secure_player_prefs` rebuilds
# one case.
#
# Each case is a directory holding a single `sample.cs` and produces a
# `sample.dll` next to it. Cases that need richer setups (multiple
# sources, references) can grow into the dll-diff Makefile's shape.

ifdef CASE
CASES := $(CASE)
else
CASES := $(patsubst %/sample.cs,%,$(wildcard */sample.cs))
endif

DLLS := $(foreach c,$(CASES),$(c)/sample.dll)

.PHONY: all clean
all: $(DLLS)

BUILD := ../../../dll-diff/tests/fixtures/build-dll.sh

%/sample.dll: %/sample.cs $(BUILD)
	$(BUILD) $@ $<

clean:
	@find . -maxdepth 2 -name '*.dll' -delete
