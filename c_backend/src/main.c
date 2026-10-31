#include "app.h"
#include "config.h"
#include "http.h"
#include "util.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DEFAULT_ADDRESS "0.0.0.0:8090"

typedef struct {
    const char *address;
    unsigned int workers;
    int migrate;
    int create_admin;
    const char *create_admin_name;
} options;

static void usage(FILE *stream) {
    fprintf(stream,
            "study-platform-c %s\n"
            "Uso: study-platform-c [--addr HOST:PUERTO] [--workers N]\n"
            "                      [--migrate] [--create-admin USUARIO]\n\n"
            "Backend C en migración. --migrate y --create-admin fallan de forma\n"
            "explícita hasta que sus contratos estén portados; nunca delegan en Rust.\n",
            SP_VERSION);
}

static int parse_options(int argc, char **argv, options *result, char **error) {
    int index;
    const char *workers_env = getenv("C_WORKERS");
    result->address = DEFAULT_ADDRESS;
    result->workers = 4;
    result->migrate = 0;
    result->create_admin = 0;
    result->create_admin_name = NULL;
    if (workers_env != NULL && *workers_env != '\0') {
        char *end;
        unsigned long value = strtoul(workers_env, &end, 10);
        if (*end != '\0' || value == 0 || value > 64) {
            sp_set_error(error, "C_WORKERS debe estar entre 1 y 64");
            return -1;
        }
        result->workers = (unsigned int)value;
    }
    for (index = 1; index < argc; index++) {
        const char *arg = argv[index];
        if (strcmp(arg, "--help") == 0 || strcmp(arg, "-h") == 0) {
            usage(stdout);
            exit(0);
        }
        if (strcmp(arg, "--version") == 0) {
            printf("%s\n", SP_VERSION);
            exit(0);
        }
        if (strcmp(arg, "--migrate") == 0) {
            result->migrate = 1;
            continue;
        }
        if (strcmp(arg, "--create-admin") == 0 && index + 1 < argc) {
            result->create_admin = 1;
            result->create_admin_name = argv[++index];
            continue;
        }
        if ((strcmp(arg, "--addr") == 0 || strcmp(arg, "-a") == 0) &&
            index + 1 < argc) {
            result->address = argv[++index];
            continue;
        }
        if (strncmp(arg, "--addr=", 7U) == 0) {
            result->address = arg + 7;
            continue;
        }
        if (strcmp(arg, "--workers") == 0 && index + 1 < argc) {
            char *end;
            unsigned long value = strtoul(argv[++index], &end, 10);
            if (*end != '\0' || value == 0 || value > 64) {
                sp_set_error(error, "--workers debe estar entre 1 y 64");
                return -1;
            }
            result->workers = (unsigned int)value;
            continue;
        }
        sp_set_error(error, "argumento desconocido o incompleto: %s", arg);
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    options cli;
    sp_config config;
    sp_app app;
    const char *static_root;
    char *error = NULL;
    int app_initialized = 0;
    int exit_code = 1;

    sp_config_init(&config);
    if (parse_options(argc, argv, &cli, &error) != 0) {
        goto done;
    }
    if (cli.migrate || cli.create_admin) {
        sp_set_error(&error,
                     "esa operación aún no está migrada a C; el binario Rust sigue "
                     "siendo la referencia y no se ha ejecutado ningún cambio");
        goto done;
    }
    if (sp_config_load_dotenv(".env", &error) != 0 ||
        sp_config_from_env(&config, &error) != 0) {
        goto done;
    }
    static_root = getenv("SP_STATIC_ROOT");
    if (static_root == NULL || *static_root == '\0') {
        static_root = ".";
    }
    if (sp_app_init(&app, &config, static_root, &error) != 0) {
        goto done;
    }
    app_initialized = 1;
    if (sp_http_serve(cli.address, cli.workers, sp_app_handle, &app, &error) != 0) {
        goto done;
    }
    exit_code = 0;

done:
    if (app_initialized) {
        sp_app_destroy(&app);
    }
    sp_config_destroy(&config);
    if (error != NULL) {
        fprintf(stderr, "study-platform-c: %s\n", error);
        free(error);
    }
    return exit_code;
}
