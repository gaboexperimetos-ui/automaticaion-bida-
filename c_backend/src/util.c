#include "util.h"

#include <ctype.h>
#include <errno.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

char *sp_strndup(const char *text, size_t len) {
    char *copy;
    if (text == NULL) {
        return NULL;
    }
    copy = malloc(len + 1U);
    if (copy == NULL) {
        return NULL;
    }
    memcpy(copy, text, len);
    copy[len] = '\0';
    return copy;
}

char *sp_strdup(const char *text) {
    if (text == NULL) {
        return NULL;
    }
    return sp_strndup(text, strlen(text));
}

void sp_set_error(char **error, const char *format, ...) {
    va_list args;
    va_list copy;
    int needed;
    char *message;

    if (error == NULL || *error != NULL) {
        return;
    }
    va_start(args, format);
    va_copy(copy, args);
    needed = vsnprintf(NULL, 0, format, copy);
    va_end(copy);
    if (needed < 0) {
        va_end(args);
        return;
    }
    message = malloc((size_t)needed + 1U);
    if (message != NULL) {
        (void)vsnprintf(message, (size_t)needed + 1U, format, args);
        *error = message;
    }
    va_end(args);
}

void sp_buffer_init(sp_buffer *buffer) {
    buffer->data = NULL;
    buffer->len = 0;
    buffer->cap = 0;
}

void sp_buffer_destroy(sp_buffer *buffer) {
    free(buffer->data);
    sp_buffer_init(buffer);
}

static int sp_buffer_reserve(sp_buffer *buffer, size_t extra) {
    size_t required;
    size_t capacity;
    char *grown;

    if (extra > SIZE_MAX - buffer->len - 1U) {
        return -1;
    }
    required = buffer->len + extra + 1U;
    if (required <= buffer->cap) {
        return 0;
    }
    capacity = buffer->cap == 0 ? 256U : buffer->cap;
    while (capacity < required) {
        if (capacity > SIZE_MAX / 2U) {
            capacity = required;
            break;
        }
        capacity *= 2U;
    }
    grown = realloc(buffer->data, capacity);
    if (grown == NULL) {
        return -1;
    }
    buffer->data = grown;
    buffer->cap = capacity;
    if (buffer->len == 0) {
        buffer->data[0] = '\0';
    }
    return 0;
}

int sp_buffer_append_n(sp_buffer *buffer, const char *text, size_t len) {
    if (text == NULL || sp_buffer_reserve(buffer, len) != 0) {
        return -1;
    }
    if (len > 0) {
        memcpy(buffer->data + buffer->len, text, len);
    }
    buffer->len += len;
    buffer->data[buffer->len] = '\0';
    return 0;
}

int sp_buffer_append(sp_buffer *buffer, const char *text) {
    if (text == NULL) {
        return -1;
    }
    return sp_buffer_append_n(buffer, text, strlen(text));
}

int sp_buffer_append_char(sp_buffer *buffer, char value) {
    return sp_buffer_append_n(buffer, &value, 1U);
}

int sp_buffer_appendf(sp_buffer *buffer, const char *format, ...) {
    va_list args;
    va_list copy;
    int needed;
    int result;

    va_start(args, format);
    va_copy(copy, args);
    needed = vsnprintf(NULL, 0, format, copy);
    va_end(copy);
    if (needed < 0 || sp_buffer_reserve(buffer, (size_t)needed) != 0) {
        va_end(args);
        return -1;
    }
    result = vsnprintf(buffer->data + buffer->len, (size_t)needed + 1U,
                       format, args);
    va_end(args);
    if (result != needed) {
        return -1;
    }
    buffer->len += (size_t)needed;
    return 0;
}

int sp_buffer_json_string(sp_buffer *buffer, const char *text) {
    const unsigned char *cursor = (const unsigned char *)text;
    if (text == NULL) {
        return sp_buffer_append(buffer, "null");
    }
    if (sp_buffer_append_char(buffer, '"') != 0) {
        return -1;
    }
    while (*cursor != '\0') {
        char escape[7];
        const char *sequence = NULL;
        switch (*cursor) {
        case '"':
            sequence = "\\\"";
            break;
        case '\\':
            sequence = "\\\\";
            break;
        case '\b':
            sequence = "\\b";
            break;
        case '\f':
            sequence = "\\f";
            break;
        case '\n':
            sequence = "\\n";
            break;
        case '\r':
            sequence = "\\r";
            break;
        case '\t':
            sequence = "\\t";
            break;
        default:
            break;
        }
        if (sequence != NULL) {
            if (sp_buffer_append(buffer, sequence) != 0) {
                return -1;
            }
        } else if (*cursor < 0x20U) {
            (void)snprintf(escape, sizeof(escape), "\\u%04x", *cursor);
            if (sp_buffer_append_n(buffer, escape, 6U) != 0) {
                return -1;
            }
        } else if (sp_buffer_append_char(buffer, (char)*cursor) != 0) {
            return -1;
        }
        cursor++;
    }
    return sp_buffer_append_char(buffer, '"');
}

char *sp_buffer_take(sp_buffer *buffer) {
    char *result;
    if (buffer->data == NULL) {
        result = sp_strdup("");
    } else {
        result = buffer->data;
    }
    sp_buffer_init(buffer);
    return result;
}

unsigned char *sp_read_file(const char *path, size_t max_size, size_t *size,
                            char **error) {
    FILE *file;
    unsigned char *data;
    long end;
    size_t read_size;

    *size = 0;
    file = fopen(path, "rb");
    if (file == NULL) {
        sp_set_error(error, "%s: %s", path, strerror(errno));
        return NULL;
    }
    if (fseek(file, 0, SEEK_END) != 0 || (end = ftell(file)) < 0 ||
        fseek(file, 0, SEEK_SET) != 0) {
        sp_set_error(error, "no se pudo medir %s", path);
        (void)fclose(file);
        return NULL;
    }
    if ((unsigned long)end > max_size) {
        sp_set_error(error, "%s supera el límite de %zu bytes", path, max_size);
        (void)fclose(file);
        return NULL;
    }
    data = malloc((size_t)end + 1U);
    if (data == NULL) {
        sp_set_error(error, "memoria insuficiente para %s", path);
        (void)fclose(file);
        return NULL;
    }
    read_size = fread(data, 1, (size_t)end, file);
    if (read_size != (size_t)end || ferror(file)) {
        sp_set_error(error, "no se pudo leer %s", path);
        free(data);
        (void)fclose(file);
        return NULL;
    }
    (void)fclose(file);
    data[read_size] = '\0';
    *size = read_size;
    return data;
}

char *sp_rfc3339_now(void) {
    time_t now = time(NULL);
    struct tm utc;
    char text[32];

    if (now == (time_t)-1 || gmtime_r(&now, &utc) == NULL ||
        strftime(text, sizeof(text), "%Y-%m-%dT%H:%M:%SZ", &utc) == 0) {
        return NULL;
    }
    return sp_strdup(text);
}

int sp_mkdir_parent(const char *path, char **error) {
    char *copy = sp_strdup(path);
    char *cursor;
    struct stat info;

    if (copy == NULL) {
        sp_set_error(error, "memoria insuficiente");
        return -1;
    }
    for (cursor = copy + 1; *cursor != '\0'; cursor++) {
        if (*cursor != '/') {
            continue;
        }
        *cursor = '\0';
        if (*copy != '\0' && mkdir(copy, 0755) != 0 && errno != EEXIST) {
            sp_set_error(error, "mkdir %s: %s", copy, strerror(errno));
            free(copy);
            return -1;
        }
        *cursor = '/';
    }
    cursor = strrchr(copy, '/');
    if (cursor != NULL) {
        *cursor = '\0';
        if (*copy != '\0' && stat(copy, &info) == 0 && !S_ISDIR(info.st_mode)) {
            sp_set_error(error, "%s no es un directorio", copy);
            free(copy);
            return -1;
        }
    }
    free(copy);
    return 0;
}
