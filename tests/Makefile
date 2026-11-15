# Linux host tests for common/ and engine/:
#   make check    build and run them
#   make golden   regenerate golden/ after an intended change (and read the diff)
CC       ?= cc
CFLAGS   ?= -std=gnu11 -O1 -g -Wall -Wextra -Wno-unused-parameter -Wno-missing-field-initializers
CPPFLAGS += -DOZZY_USER_ENGINE -I..
LDLIBS   += -lm    # the float codecs (fminf, fmaxf)
BUILD    := build
ENGINE   := ../engine/core/ozzy_engine.c \
            ../engine/devices/ploytec/ploytec_engine.c ../engine/devices/ploytec/ploytec_io.c \
            ../common/devices/ploytec/ploytec_handshake.c ../common/devices/ploytec/ploytec_codec.c
TESTS    := test_endpoints test_settings test_info test_transfers test_trace test_routing test_memory test_layout test_stats test_latency test_cport
HEADERS  := $(wildcard ../engine/*/*.h ../engine/devices/*/*.h ../common/*.h ../common/devices/*/*.h) fake_bus.h check.h

all: $(addprefix $(BUILD)/,$(TESTS))

$(BUILD):
	mkdir -p $@

$(BUILD)/%: %.c fake_bus.c $(ENGINE) $(HEADERS) | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -o $@ $< fake_bus.c $(ENGINE) $(LDLIBS)

check: all
	@./run.sh $(BUILD)

golden: all
	$(BUILD)/test_trace > golden/trace.txt 2>/dev/null
	$(BUILD)/test_layout > golden/layout.txt

clean:
	rm -rf $(BUILD)

.PHONY: all check golden clean
