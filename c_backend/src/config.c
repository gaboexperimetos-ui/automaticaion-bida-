#include "config.h"

#include "util.h"

#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

#define DEFAULT_MODEL "cohere/north-mini-code:free"

static void sp_config_clear_values(sp_config *config) {
    free(config->host);
    free(config->session_secret);
    free(config->db_path);
    free(config->notes_dir);
    free(config->openrouter_api_key);
    free(config->openrouter_model);
    free(config->mikareno_mcp_url);
    free(config->genoffice_mcp_url);
    free(config->custom_mcp_api_key);
    free(config->rust_log);
    free(config->registration_token);
    sp_config_init(config);
}

void sp_config_init(sp_config *config) {
    memset(config, 0, sizeof(*config));
}

void sp_config_destroy(sp_config *config) {
    sp_config_clear_values(config);
}

static char *trim(char *text) {
    char *end;
    while (isspace((unsigned char)*text)) {
        text++;
    }
    if (*text == '\0') {
        return text;
    }
    end = text + strlen(text) - 1;
    while (end >= text && isspace((unsigned char)*end)) {
        *end = '\0';
        end--;
    }
    return text;
}

static int valid_env_key(const char *key) {
    size_t index;
    if (*key == '\0' || !(isalpha((unsigned char)*key) || *key == '_')) {
        return 0;
    }
    for (index = 1; key[index] != '\0'; index++) {
        if (!(isalnum((unsigned char)key[index]) || key[index] == '_')) {
            return 0;
        }
    }
    return 1;
}

static char *unquote_env_value(char *value) {
    char quote = *value;
    char *read_cursor;
    char *write_cursor;

    if (quote != '\'' && quote != '"') {
        return trim(value);
    }
    read_cursor = value + 1;
    write_cursor = value;
    while (*read_cursor != '\0' && *read_cursor != quote) {
        if (quote == '"' && *read_cursor == '\\' && read_cursor[1] != '\0') {
            read_cursor++;
            switch (*read_cursor) {
            case 'n':
                *write_cursor = '\n';
                break;
            case 'r':
                *write_cursor = '\r';
                break;
            case 't':
                *write_cursor = '\t';
                break;
            default:
                *write_cursor = *read_cursor;
                break;
            }
        } else {
            *write_cursor = *read_cursor;
        }
        write_cursor++;
        read_cursor++;
    }
    if (*read_cursor != quote) {
        return NULL;
    }
    *write_cursor = '\0';
    return value;
}

int sp_config_load_dotenv(const char *path, char **error) {
    FILE *file = fopen(path, "r");
    char *line = NULL;
    size_t capacity = 0;
    ssize_t length;
    size_t line_number = 0;
    int result = 0;

    if (file == NULL) {
        if (errno == ENOENT) {
            return 0;
        }
        sp_set_error(error, "%s: %s", path, strerror(errno));
        return -1;
    }
    while ((length = getline(&line, &capacity, file)) >= 0) {
        char *cursor;
        char *equals;
        char *key;
        char *value;
        line_number++;
        (void)length;
        cursor = trim(line);
        if (*cursor == '\0' || *cursor == '#') {
            continue;
        }
        if (strncmp(cursor, "export ", 7U) == 0) {
            cursor = trim(cursor + 7);
        }
        equals = strchr(cursor, '=');
        if (equals == NULL) {
            sp_set_error(error, "%s:%zu: falta '='", path, line_number);
            result = -1;
            break;
        }
        *equals = '\0';
        key = trim(cursor);
        value = trim(equals + 1);
        if (!valid_env_key(key)) {
            sp_set_error(error, "%s:%zu: variable inválida", path, line_number);
            result = -1;
            break;
        }
        value = unquote_env_value(value);
        if (value == NULL) {
            sp_set_error(error, "%s:%zu: comillas sin cerrar", path, line_number);
            result = -1;
            break;
        }
        if (getenv(key) == NULL && setenv(key, value, 0) != 0) {
            sp_set_error(error, "setenv %s: %s", key, strerror(errno));
            result = -1;
            break;
        }
    }
    if (ferror(file) && result == 0) {
        sp_set_error(error, "error leyendo %s", path);
        result = -1;
    }
    free(line);
    (void)fclose(file);
    return result;
}

static int copy_env(char **target, const char *key, const char *fallback,
                    bool required, char **error) {
    const char *value = getenv(key);
    if (value == NULL) {
        value = fallback;
    }
    if (required && value == NULL) {
        sp_set_error(error, "falta la variable obligatoria %s", key);
        return -1;
    }
    if (value != NULL) {
        *target = sp_strdup(value);
        if (*target == NULL) {
            sp_set_error(error, "memoria insuficiente leyendo %s", key);
            return -1;
        }
    }
    return 0;
}

static int parse_u64(const char *key, uint64_t min, uint64_t max,
                     uint64_t *target, char **error) {
    const char *value = getenv(key);
    char *end;
    unsigned long long parsed;
    if (value == NULL) {
        return 0;
    }
    errno = 0;
    parsed = strtoull(value, &end, 10);
    if (errno != 0 || *value == '\0' || *value == '-' || *end != '\0' ||
        parsed < min || parsed > max) {
        sp_set_error(error, "%s debe ser un entero entre %llu y %llu", key,
                     (unsigned long long)min, (unsigned long long)max);
        return -1;
    }
    *target = (uint64_t)parsed;
    return 0;
}

static int parse_double(const char *key, double *target, char **error) {
    const char *value = getenv(key);
    char *end;
    double parsed;
    if (value == NULL) {
        return 0;
    }
    errno = 0;
    parsed = strtod(value, &end);
    if (errno != 0 || *value == '\0' || *end != '\0' || !isfinite(parsed)) {
        sp_set_error(error, "%s debe ser un número válido", key);
        return -1;
    }
    *target = parsed;
    return 0;
}

static char *optional_trimmed_env(const char *key, char **error) {
    const char *value = getenv(key);
    char *copy;
    char *start;
    char *end;
    if (value == NULL) {
        return NULL;
    }
    copy = sp_strdup(value);
    if (copy == NULL) {
        sp_set_error(error, "memoria insuficiente leyendo %s", key);
        return NULL;
    }
    start = copy;
    while (isspace((unsigned char)*start)) {
        start++;
    }
    end = start + strlen(start);
    while (end > start && isspace((unsigned char)end[-1])) {
        end--;
    }
    *end = '\0';
    if (*start == '\0') {
        free(copy);
        return NULL;
    }
    if (start != copy) {
        memmove(copy, start, strlen(start) + 1U);
    }
    return copy;
}

static bool env_truth(const char *key) {
    const char *value = getenv(key);
    if (value == NULL) {
        return false;
    }
    return strcasecmp(value, "1") == 0 || strcasecmp(value, "true") == 0 ||
           strcasecmp(value, "yes") == 0 || strcasecmp(value, "on") == 0;
}

int sp_config_from_env(sp_config *config, char **error) {
    uint64_t number;
    int result = -1;

    sp_config_clear_values(config);
    config->port = 8090;
    config->openrouter_max_tokens = 8192;
    config->openrouter_max_cost_usd = 2.0;
    config->db_max_connections = 4;
    config->mcp_timeout_ms = 5000;

    if (copy_env(&config->host, "HOST", "127.0.0.1", false, error) != 0 ||
        copy_env(&config->session_secret, "SESSION_SECRET", NULL, true, error) !=
            0 ||
        copy_env(&config->db_path, "DB_PATH", "./data/app.db", false, error) !=
            0 ||
        copy_env(&config->notes_dir, "NOTES_DIR", "./notes", false, error) !=
            0 ||
        copy_env(&config->openrouter_model, "OPENROUTER_MODEL", DEFAULT_MODEL,
                 false, error) != 0 ||
        copy_env(&config->mikareno_mcp_url, "MIKARENO_MCP_URL",
                 "http://127.0.0.1:8080/mcp", false, error) != 0 ||
        copy_env(&config->genoffice_mcp_url, "GENOFFICE_MCP_URL",
                 "http://127.0.0.1:8453/mcp", false, error) != 0 ||
        copy_env(&config->rust_log, "RUST_LOG",
                 "study_platform=info,tower_http=info", false, error) != 0) {
        goto done;
    }
    if (strlen(config->session_secret) < 32U) {
        sp_set_error(error, "SESSION_SECRET debe tener al menos 32 caracteres");
        goto done;
    }
    config->openrouter_api_key = optional_trimmed_env("OPENROUTER_API_KEY", error);
    if (*error != NULL) {
        goto done;
    }
    config->custom_mcp_api_key = optional_trimmed_env("CUSTOM_MCP_API_KEY", error);
    if (*error != NULL) {
        goto done;
    }
    config->registration_token = optional_trimmed_env("REGISTRATION_TOKEN", error);
    if (*error != NULL) {
        goto done;
    }

    number = config->port;
    if (parse_u64("PORT", 0, 65535, &number, error) != 0) {
        goto done;
    }
    config->port = (uint16_t)number;
    number = config->openrouter_max_tokens;
    if (parse_u64("OPENROUTER_MAX_TOKENS", 0, UINT32_MAX, &number, error) != 0) {
        goto done;
    }
    config->openrouter_max_tokens = (uint32_t)number;
    number = config->db_max_connections;
    if (parse_u64("DB_MAX_CONNECTIONS", 0, UINT32_MAX, &number, error) != 0) {
        goto done;
    }
    if (number == 0) {
        number = 1;
    }
    config->db_max_connections = (uint32_t)number;
    number = config->mcp_timeout_ms;
    if (parse_u64("MCP_TIMEOUT_MS", 0, UINT64_MAX, &number, error) != 0) {
        goto done;
    }
    config->mcp_timeout_ms = number;
    if (parse_double("OPENROUTER_MAX_COST_USD",
                     &config->openrouter_max_cost_usd, error) != 0) {
        goto done;
    }
    config->auth_disabled = env_truth("AUTH_DISABLED");
    result = 0;

done:
    if (result != 0) {
        sp_config_clear_values(config);
    }
    return result;
}

char *sp_config_public_json(const sp_config *config, const char *version) {
    sp_buffer json;
    char *result = NULL;
    sp_buffer_init(&json);
    if (sp_buffer_append(&json, "{\"openrouter_configured\":") != 0 ||
        sp_buffer_append(&json,
                         config->openrouter_api_key != NULL ? "true" : "false") !=
            0 ||
        sp_buffer_append(&json, ",\"openrouter_model\":") != 0 ||
        sp_buffer_json_string(&json, config->openrouter_model) != 0 ||
        sp_buffer_append(&json, ",\"mikareno_mcp_url\":") != 0 ||
        sp_buffer_json_string(&json, config->mikareno_mcp_url) != 0 ||
        sp_buffer_append(&json, ",\"genoffice_mcp_url\":") != 0 ||
        sp_buffer_json_string(&json, config->genoffice_mcp_url) != 0 ||
        sp_buffer_append(&json, ",\"version\":") != 0 ||
        sp_buffer_json_string(&json, version) != 0 ||
        sp_buffer_appendf(&json, ",\"is_localhost\":%s}",
                          strcmp(config->host, "127.0.0.1") == 0 ? "true"
                                                                 : "false") !=
            0) {
        sp_buffer_destroy(&json);
        return NULL;
    }
    result = sp_buffer_take(&json);
    return result;
}
