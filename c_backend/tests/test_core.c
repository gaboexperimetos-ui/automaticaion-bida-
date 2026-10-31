#include "config.h"
#include "db.h"
#include "probe.h"
#include "util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int checks = 0;
static int failures = 0;

#define CHECK(condition)                                                        \
    do {                                                                        \
        checks++;                                                               \
        if (!(condition)) {                                                     \
            failures++;                                                         \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        }                                                                       \
    } while (0)

static const char *env_keys[] = {
    "HOST", "PORT", "SESSION_SECRET", "DB_PATH", "NOTES_DIR",
    "OPENROUTER_API_KEY", "OPENROUTER_MODEL", "OPENROUTER_MAX_TOKENS",
    "OPENROUTER_MAX_COST_USD", "MIKARENO_MCP_URL", "GENOFFICE_MCP_URL",
    "CUSTOM_MCP_API_KEY", "RUST_LOG", "AUTH_DISABLED", "REGISTRATION_TOKEN",
    "DB_MAX_CONNECTIONS", "MCP_TIMEOUT_MS",
};

static void reset_environment(void) {
    size_t index;
    for (index = 0; index < sizeof(env_keys) / sizeof(env_keys[0]); index++) {
        (void)unsetenv(env_keys[index]);
    }
}

static void test_buffer(void) {
    sp_buffer buffer;
    char *json;
    sp_buffer_init(&buffer);
    CHECK(sp_buffer_append(&buffer, "texto:") == 0);
    CHECK(sp_buffer_json_string(&buffer, "a\n\"b\"\\c\001") == 0);
    json = sp_buffer_take(&buffer);
    CHECK(json != NULL);
    CHECK(strcmp(json, "texto:\"a\\n\\\"b\\\"\\\\c\\u0001\"") == 0);
    free(json);
    sp_buffer_destroy(&buffer);
}

static void test_config(void) {
    sp_config config;
    char *error = NULL;
    char *json;
    char path[128];
    FILE *file;

    reset_environment();
    sp_config_init(&config);
    CHECK(sp_config_from_env(&config, &error) != 0);
    CHECK(error != NULL && strstr(error, "SESSION_SECRET") != NULL);
    free(error);
    error = NULL;

    CHECK(setenv("SESSION_SECRET", "corto", 1) == 0);
    CHECK(sp_config_from_env(&config, &error) != 0);
    CHECK(error != NULL && strstr(error, "32") != NULL);
    free(error);
    error = NULL;

    snprintf(path, sizeof(path), "/tmp/study-platform-config-%ld.env", (long)getpid());
    file = fopen(path, "w");
    CHECK(file != NULL);
    if (file != NULL) {
        fputs("PORT=9000\nDB_MAX_CONNECTIONS=0\nOPENROUTER_API_KEY=\"   \"\n",
              file);
        (void)fclose(file);
    }
    CHECK(setenv("SESSION_SECRET",
                 "0123456789abcdef0123456789abcdef-environment", 1) == 0);
    CHECK(sp_config_load_dotenv(path, &error) == 0);
    CHECK(error == NULL);
    CHECK(sp_config_from_env(&config, &error) == 0);
    CHECK(error == NULL);
    CHECK(config.port == 9000);
    CHECK(config.db_max_connections == 1);
    CHECK(config.openrouter_api_key == NULL);
    CHECK(strcmp(config.session_secret,
                 "0123456789abcdef0123456789abcdef-environment") == 0);
    json = sp_config_public_json(&config, "0.1.0");
    CHECK(json != NULL);
    CHECK(json != NULL && strstr(json, "environment") == NULL);
    CHECK(json != NULL && strstr(json, "\"openrouter_configured\":false") != NULL);
    CHECK(json != NULL && strstr(json, "\"is_localhost\":true") != NULL);
    free(json);
    sp_config_destroy(&config);

    CHECK(setenv("MCP_TIMEOUT_MS", "-1", 1) == 0);
    CHECK(sp_config_from_env(&config, &error) != 0);
    CHECK(error != NULL);
    free(error);
    error = NULL;
    (void)unlink(path);
    sp_config_destroy(&config);
    reset_environment();
}

static void test_database(void) {
    sp_db db;
    char path[128];
    char *error = NULL;
    snprintf(path, sizeof(path), "/tmp/study-platform-core-%ld.db", (long)getpid());
    (void)unlink(path);
    sp_db_init(&db);
    CHECK(sp_db_open(&db, path, &error) == 0);
    CHECK(error == NULL);
    CHECK(sp_db_ping(&db));
    sp_db_close(&db);
    CHECK(!sp_db_ping(&db));
    free(error);
    (void)unlink(path);
    CHECK(!sp_probe_mcp_http("https://127.0.0.1/mcp", 1));
    CHECK(!sp_probe_mcp_http("not-a-url", 1));
}

int main(void) {
    test_buffer();
    test_config();
    test_database();
    if (failures != 0) {
        fprintf(stderr, "%d de %d comprobaciones fallaron\n", failures, checks);
        return 1;
    }
    printf("test_core: %d comprobaciones en verde\n", checks);
    return 0;
}
