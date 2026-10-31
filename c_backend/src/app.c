#include "app.h"

#include "probe.h"
#include "util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct {
    const char *route;
    const char *relative_path;
    const char *content_type;
    int no_cache;
} static_asset;

static const static_asset assets[] = {
    {"/legacy", "static/study-platform-ui/index.html", "text/html; charset=utf-8", 0},
    {"/styles.css", "static/study-platform-ui/styles.css", "text/css; charset=utf-8", 0},
    {"/app.js", "static/study-platform-ui/app.js", "application/javascript; charset=utf-8", 0},
    {"/shell.js", "static/app/shell.js", "application/javascript; charset=utf-8", 1},
    {"/polish.css", "static/app/polish.css", "text/css; charset=utf-8", 1},
    {"/nova.css", "static/app/nova.css", "text/css; charset=utf-8", 1},
    {"/favicon.ico", "static/app/favicon.svg", "image/svg+xml", 0},
    {"/favicon.svg", "static/app/favicon.svg", "image/svg+xml", 0},
};

int sp_app_init(sp_app *app, const sp_config *config, const char *static_root,
                char **error) {
    memset(app, 0, sizeof(*app));
    app->config = config;
    app->static_root = static_root;
    sp_db_init(&app->db);
    if (pthread_mutex_init(&app->probe_mutex, NULL) != 0) {
        sp_set_error(error, "no se pudo iniciar la caché de health");
        return -1;
    }
    if (sp_db_open(&app->db, config->db_path, error) != 0) {
        (void)pthread_mutex_destroy(&app->probe_mutex);
        return -1;
    }
    return 0;
}

void sp_app_destroy(sp_app *app) {
    sp_db_close(&app->db);
    (void)pthread_mutex_destroy(&app->probe_mutex);
    app->config = NULL;
    app->static_root = NULL;
}

static void mcp_components(sp_app *app, bool *mikareno, bool *genoffice) {
    time_t now = time(NULL);
    pthread_mutex_lock(&app->probe_mutex);
    if (app->probe_checked_at == 0 || now < app->probe_checked_at ||
        now - app->probe_checked_at >= 30) {
        /* Sondeos HTTP reales, con el mismo TTL de 30 s que el backend Rust.
         * HTTPS/TLS queda como una dependencia explícita de una fase posterior. */
        app->mikareno_ok =
            sp_probe_mcp_http(app->config->mikareno_mcp_url, 2500);
        app->genoffice_ok =
            sp_probe_mcp_http(app->config->genoffice_mcp_url, 2500);
        app->probe_checked_at = time(NULL);
    }
    *mikareno = app->mikareno_ok;
    *genoffice = app->genoffice_ok;
    pthread_mutex_unlock(&app->probe_mutex);
}

static sp_http_response health(sp_app *app) {
    bool database = sp_db_ping(&app->db);
    bool mikareno;
    bool genoffice;
    char *timestamp = sp_rfc3339_now();
    sp_buffer json;
    sp_http_response response;

    if (timestamp == NULL) {
        return sp_http_text(500, "Internal error");
    }
    mcp_components(app, &mikareno, &genoffice);
    sp_buffer_init(&json);
    if (sp_buffer_appendf(&json, "{\"status\":\"%s\",\"timestamp\":",
                          database ? "healthy" : "degraded") != 0 ||
        sp_buffer_json_string(&json, timestamp) != 0 ||
        sp_buffer_appendf(&json, ",\"version\":\"%s\",\"components\":{", SP_VERSION) !=
            0 ||
        sp_buffer_appendf(&json, "\"database\":%s,", database ? "true" : "false") !=
            0 ||
        sp_buffer_appendf(&json, "\"openrouter\":%s,",
                          app->config->openrouter_api_key != NULL ? "true" : "false") !=
            0 ||
        sp_buffer_appendf(&json, "\"mikareno_mcp\":%s,", mikareno ? "true" : "false") !=
            0 ||
        sp_buffer_appendf(&json, "\"genoffice_mcp\":%s}}",
                          genoffice ? "true" : "false") != 0) {
        free(timestamp);
        sp_buffer_destroy(&json);
        return sp_http_text(500, "Internal error");
    }
    free(timestamp);
    response = sp_http_json(database ? 200 : 503, sp_buffer_take(&json));
    return response;
}

static sp_http_response serve_asset(const sp_app *app,
                                    const static_asset *asset) {
    sp_buffer path;
    unsigned char *content;
    size_t size;
    char *error = NULL;
    sp_http_response response;

    sp_buffer_init(&path);
    if (sp_buffer_append(&path, app->static_root) != 0 ||
        sp_buffer_append_char(&path, '/') != 0 ||
        sp_buffer_append(&path, asset->relative_path) != 0) {
        sp_buffer_destroy(&path);
        return sp_http_text(500, "Internal error");
    }
    content = sp_read_file(path.data, 16U * 1024U * 1024U, &size, &error);
    sp_buffer_destroy(&path);
    if (content == NULL) {
        free(error);
        return sp_http_text(404, "Not found");
    }
    response.status = 200;
    response.content_type = asset->content_type;
    response.body = content;
    response.body_len = size;
    response.no_cache = asset->no_cache;
    return response;
}

static int method_is_get(const sp_http_request *request) {
    return strcmp(request->method, "GET") == 0;
}

sp_http_response sp_app_handle(void *context, const sp_http_request *request) {
    sp_app *app = context;
    size_t index;

    if (strcmp(request->path, "/health") == 0) {
        if (!method_is_get(request)) {
            return sp_http_text(405, "Method not allowed");
        }
        return health(app);
    }
    if (strcmp(request->path, "/api/config") == 0 ||
        strcmp(request->path, "/api/public-config") == 0) {
        if (!method_is_get(request)) {
            return sp_http_text(405, "Method not allowed");
        }
        return sp_http_json(
            200, sp_config_public_json(app->config, SP_VERSION));
    }
    for (index = 0; index < sizeof(assets) / sizeof(assets[0]); index++) {
        if (strcmp(request->path, assets[index].route) == 0) {
            if (!method_is_get(request)) {
                return sp_http_text(405, "Method not allowed");
            }
            return serve_asset(app, &assets[index]);
        }
    }
    return sp_http_text(404, "Not found");
}
