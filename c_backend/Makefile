CC ?= cc
PYTHON ?= python3

CPPFLAGS := -D_POSIX_C_SOURCE=200809L -Iinclude
WARNINGS := -Wall -Wextra -Wpedantic -Werror -Wshadow -Wformat=2 -Wstrict-prototypes
BASE_CFLAGS := -std=c17 $(WARNINGS) -fno-omit-frame-pointer -fstack-protector-strong
CFLAGS ?= -O2 -g
# Se enlaza el runtime por SONAME; no se requieren cabeceras ni enlaces -dev.
LDLIBS := -l:libsqlite3.so.0 -pthread

SOURCES := src/main.c src/app.c src/config.c src/db.c src/http.c src/probe.c src/util.c
CORE_TEST_SOURCES := tests/test_core.c src/config.c src/db.c src/probe.c src/util.c
HEADERS := $(wildcard include/*.h)
OBJECTS := $(SOURCES:src/%.c=build/%.o)
DEPS := $(OBJECTS:.o=.d)
ANALYZER_OBJECTS := $(SOURCES:src/%.c=build/analyzer/%.o)
APP := build/study-platform-c
TEST_APP := build/study-platform-c-sanitize
TEST_CORE := build/test-core-sanitize
SANITIZERS := -fsanitize=address,undefined

.PHONY: all clean test analyze ci sanitize

all: $(APP)

$(APP): $(OBJECTS)
	$(CC) $(LDFLAGS) $^ $(LDLIBS) -o $@

build/%.o: src/%.c | build
	$(CC) $(CPPFLAGS) $(BASE_CFLAGS) $(CFLAGS) -MMD -MP -c $< -o $@

build:
	mkdir -p build

build/analyzer:
	mkdir -p build/analyzer

build/analyzer/%.o: src/%.c $(HEADERS) | build/analyzer
	$(CC) $(CPPFLAGS) $(BASE_CFLAGS) -O0 -fanalyzer -c $< -o $@

$(TEST_APP): $(SOURCES) $(HEADERS) | build
	$(CC) $(CPPFLAGS) $(BASE_CFLAGS) -O1 -g $(SANITIZERS) $(SOURCES) $(LDLIBS) $(SANITIZERS) -o $@

$(TEST_CORE): $(CORE_TEST_SOURCES) $(HEADERS) | build
	$(CC) $(CPPFLAGS) $(BASE_CFLAGS) -O1 -g $(SANITIZERS) $(CORE_TEST_SOURCES) $(LDLIBS) $(SANITIZERS) -o $@

sanitize: $(TEST_APP) $(TEST_CORE)

test: sanitize
	ASAN_OPTIONS=detect_leaks=1:strict_string_checks=1 UBSAN_OPTIONS=halt_on_error=1 ./$(TEST_CORE)
	ASAN_OPTIONS=detect_leaks=1:strict_string_checks=1 UBSAN_OPTIONS=halt_on_error=1 SP_C_BIN=$(abspath $(TEST_APP)) $(PYTHON) tests/integration.py

analyze: $(ANALYZER_OBJECTS)

ci: all test analyze

clean:
	rm -rf build

-include $(DEPS)
