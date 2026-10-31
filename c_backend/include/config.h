#ifndef SP_CONFIG_H
#define SP_CONFIG_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    char *host;
    uint16_t port;
    char *session_secret;
    char *db_path;
    char *notes_dir;
    char *openrouter_api_key;
    char *openrouter_model;
    uint32_t openrouter_max_tokens;
    double openrouter_max_cost_usd;
    char *mikareno_mcp_url;
    char *genoffice_mcp_url;
    char *custom_mcp_api_key;
    char *rust_log;
    bool auth_disabled;
    char *registration_token;
    uint32_t db_max_connections;
    uint64_t mcp_timeout_ms;
} sp_config;

void sp_config_init(sp_config *config);
void sp_config_destroy(sp_config *config);
int sp_config_load_dotenv(const char *path, char **error);
int sp_config_from_env(sp_config *config, char **error);
char *sp_config_public_json(const sp_config *config, const char *version);

#endif
