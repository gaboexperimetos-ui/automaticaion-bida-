#ifndef SP_UTIL_H
#define SP_UTIL_H

#include <stddef.h>

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} sp_buffer;

void sp_buffer_init(sp_buffer *buffer);
void sp_buffer_destroy(sp_buffer *buffer);
int sp_buffer_append_n(sp_buffer *buffer, const char *text, size_t len);
int sp_buffer_append(sp_buffer *buffer, const char *text);
int sp_buffer_append_char(sp_buffer *buffer, char value);
int sp_buffer_appendf(sp_buffer *buffer, const char *format, ...);
int sp_buffer_json_string(sp_buffer *buffer, const char *text);
char *sp_buffer_take(sp_buffer *buffer);

char *sp_strdup(const char *text);
char *sp_strndup(const char *text, size_t len);
void sp_set_error(char **error, const char *format, ...);
unsigned char *sp_read_file(const char *path, size_t max_size, size_t *size,
                            char **error);
char *sp_rfc3339_now(void);
int sp_mkdir_parent(const char *path, char **error);

#endif
