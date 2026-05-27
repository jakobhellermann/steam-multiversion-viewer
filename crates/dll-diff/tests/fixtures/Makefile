# Regenerate from.dll / to.dll (and optionally lib.dll) for every
# fixture case under this directory. Incremental: only rebuilds DLLs
# whose sources changed.
#
# Usage: make                # build everything that's stale
#        make CASE=add_type  # build a single case (and its lib if any)
#        make clean
#
# A case with a `lib.cs` produces three DLLs in that order:
#   lib.dll   shared type built from lib.cs alone
#   from.dll  lib.cs inlined as a `Compile` → the type is a
#             TypeDefinition inside from.dll
#   to.dll    lib.dll attached as a `Reference` → the same type is a
#             TypeReference in to.dll
# The asymmetry is what we need to reproduce the Definition vs.
# Reference cascade false positive.

ifdef CASE
CASES := $(CASE)
else
CASES := $(patsubst %/from.cs,%,$(wildcard */from.cs))
endif

DLLS     := $(foreach c,$(CASES),$(c)/from.dll $(c)/to.dll)
LIB_DLLS := $(foreach c,$(CASES),$(if $(wildcard $(c)/lib.cs),$(c)/lib.dll))

.PHONY: all clean
all: $(DLLS) $(LIB_DLLS)

BUILD := ./build-dll.sh

define case_rules
ifneq ($$(wildcard $(1)/lib.cs),)
$(1)/lib.dll: $(1)/lib.cs $(BUILD)
	$$(BUILD) $$@ $$<

$(1)/from.dll: $(1)/from.cs $(1)/lib.cs $(BUILD)
	$$(BUILD) $$@ $(1)/from.cs $(1)/lib.cs

$(1)/to.dll: $(1)/to.cs $(1)/lib.dll $(BUILD)
	$$(BUILD) $$@ $(1)/to.cs --ref $(1)/lib.dll
else
$(1)/from.dll: $(1)/from.cs $(BUILD)
	$$(BUILD) $$@ $$<

$(1)/to.dll: $(1)/to.cs $(BUILD)
	$$(BUILD) $$@ $$<
endif
endef

$(foreach c,$(CASES),$(eval $(call case_rules,$(c))))

clean:
	@find . -maxdepth 2 -name '*.dll' -delete
