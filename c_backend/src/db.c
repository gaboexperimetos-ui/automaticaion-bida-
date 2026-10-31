#include "db.h"

#include "util.h"

#include <stddef.h>
#include <string.h>

/* SQLite es una dependencia obligatoria. Se enlaza por SONAME para no exigir
 * el paquete -dev únicamente por sus cabeceras; estas declaraciones son la
 * superficie pública y estable de C ABI utilizada por este backend. */
typedef struct sqlite3_stmt sqlite3_stmt;
extern int sqlite3_open_v2(const char *, sqlite3 **, int, const char *);
extern int sqlite3_close_v2(sqlite3 *);
extern int sqlite3_prepare_v2(sqlite3 *, const char *, int, sqlite3_stmt **,
                              const char **);
extern int sqlite3_step(sqlite3_stmt *);
extern int sqlite3_column_int(sqlite3_stmt *, int);
extern int sqlite3_finalize(sqlite3_stmt *);
extern const char *sqlite3_errmsg(sqlite3 *);
extern int sqlite3_busy_timeout(sqlite3 *, int);
extern int sqlite3_exec(sqlite3 *, const char *,
                        int (*)(void *, int, char **, char **), void *, char **);
extern void sqlite3_free(void *);

#define SQLITE_OK 0
#define SQLITE_ROW 100
#define SQLITE_OPEN_READWRITE 0x00000002
#define SQLITE_OPEN_CREATE 0x00000004
#define SQLITE_OPEN_FULLMUTEX 0x00010000

void sp_db_init(sp_db *db) {
    memset(db, 0, sizeof(*db));
}

const char *sp_db_last_error(const sp_db *db) {
    if (db->connection != NULL) {
        return sqlite3_errmsg(db->connection);
    }
    return "SQLite no disponible";
}

static int exec_pragma(sp_db *db, const char *sql, char **error) {
    char *sqlite_error = NULL;
    int code = sqlite3_exec(db->connection, sql, NULL, NULL, &sqlite_error);
    if (code != SQLITE_OK) {
        sp_set_error(error, "SQLite: %s",
                     sqlite_error != NULL ? sqlite_error : sp_db_last_error(db));
    }
    if (sqlite_error != NULL) {
        sqlite3_free(sqlite_error);
    }
    return code == SQLITE_OK ? 0 : -1;
}

int sp_db_open(sp_db *db, const char *path, char **error) {
    int code;
    static const char *pragmas[] = {
        "PRAGMA foreign_keys=ON;",
        "PRAGMA cache_size=-2000;",
        "PRAGMA mmap_size=0;",
        "PRAGMA temp_store=FILE;",
        "PRAGMA wal_autocheckpoint=1000;",
    };
    size_t index;

    sp_db_close(db);
    if (sp_mkdir_parent(path, error) != 0) {
        return -1;
    }
    code = sqlite3_open_v2(path, &db->connection,
                           SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE |
                               SQLITE_OPEN_FULLMUTEX,
                           NULL);
    if (code != SQLITE_OK) {
        sp_set_error(error, "no se pudo abrir %s: %s", path,
                     sp_db_last_error(db));
        sp_db_close(db);
        return -1;
    }
    code = sqlite3_busy_timeout(db->connection, 5000);
    if (code != SQLITE_OK) {
        sp_set_error(error, "no se pudo configurar busy_timeout: %s",
                     sp_db_last_error(db));
        sp_db_close(db);
        return -1;
    }
    for (index = 0; index < sizeof(pragmas) / sizeof(pragmas[0]); index++) {
        if (exec_pragma(db, pragmas[index], error) != 0) {
            sp_db_close(db);
            return -1;
        }
    }
    return 0;
}

void sp_db_close(sp_db *db) {
    if (db->connection != NULL) {
        (void)sqlite3_close_v2(db->connection);
        db->connection = NULL;
    }
}

bool sp_db_ping(sp_db *db) {
    sqlite3_stmt *statement = NULL;
    bool ok = false;
    if (db->connection == NULL) {
        return false;
    }
    if (sqlite3_prepare_v2(db->connection, "SELECT 1", -1, &statement, NULL) ==
            SQLITE_OK &&
        statement != NULL) {
        ok = sqlite3_step(statement) == SQLITE_ROW &&
             sqlite3_column_int(statement, 0) == 1;
    }
    if (statement != NULL) {
        (void)sqlite3_finalize(statement);
    }
    return ok;
}
