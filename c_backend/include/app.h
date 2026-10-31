#ifndef SP_APP_H
#define SP_APP_H

#include "config.h"
#include "db.h"
#include "http.h"

#include <pthread.h>
#include <stdbool.h>
#include <time.h>

#define SP_VERSION "0.1.0"

typedef struct {
    const sp_config *config;
    sp_db db;
    const char *static_root;
    pthread_mutex_t probe_mutex;
    time_t probe_checked_at;
    bool mikareno_ok;
    bool genoffice_ok;
} sp_app;

int sp_app_init(sp_app *app, const sp_config *config, const char *static_root,
                char **error);
void sp_app_destroy(sp_app *app);
sp_http_response sp_app_handle(void *context, const sp_http_request *request);

#endif
