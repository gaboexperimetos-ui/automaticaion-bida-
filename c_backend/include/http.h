#ifndef SP_HTTP_H
#define SP_HTTP_H

#include <stddef.h>

#define SP_HTTP_MAX_TARGET 2048U
#define SP_HTTP_MAX_PATH 1024U
#define SP_HTTP_MAX_BODY (1024U * 1024U)
#define SP_HTTP_IP_TEXT 64U

typedef struct {
    char method[16];
    char target[SP_HTTP_MAX_TARGET];
    char path[SP_HTTP_MAX_PATH];
    const char *query;
    char remote_ip[SP_HTTP_IP_TEXT];
    unsigned char *body;
    size_t body_len;
} sp_http_request;

typedef struct {
    int status;
    const char *content_type;
    unsigned char *body;
    size_t body_len;
    int no_cache;
} sp_http_response;

typedef sp_http_response (*sp_http_handler)(void *context,
                                            const sp_http_request *request);

void sp_http_response_init(sp_http_response *response);
sp_http_response sp_http_text(int status, const char *text);
sp_http_response sp_http_json(int status, char *owned_json);
void sp_http_response_destroy(sp_http_response *response);
int sp_http_serve(const char *address, unsigned int workers,
                  sp_http_handler handler, void *context, char **error);

#endif
