#ifndef SP_DB_H
#define SP_DB_H

#include <stdbool.h>

typedef struct sqlite3 sqlite3;

typedef struct {
    sqlite3 *connection;
} sp_db;

void sp_db_init(sp_db *db);
int sp_db_open(sp_db *db, const char *path, char **error);
void sp_db_close(sp_db *db);
bool sp_db_ping(sp_db *db);
const char *sp_db_last_error(const sp_db *db);

#endif
