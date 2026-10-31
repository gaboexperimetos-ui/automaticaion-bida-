#include "http.h"

#include "util.h"

#include <arpa/inet.h>
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#define HEADER_LIMIT 16384U
#define QUEUE_LIMIT 128U
#define IO_TIMEOUT_SECONDS 10

typedef struct {
    int fd;
    char remote_ip[SP_HTTP_IP_TEXT];
} connection_job;

typedef struct {
    connection_job queue[QUEUE_LIMIT];
    size_t head;
    size_t tail;
    size_t count;
    int stopping;
    pthread_mutex_t mutex;
    pthread_cond_t available;
    pthread_cond_t space;
    pthread_t *threads;
    unsigned int thread_count;
    sp_http_handler handler;
    void *handler_context;
} worker_pool;

static volatile sig_atomic_t stop_requested = 0;
static volatile sig_atomic_t listen_fd_for_signal = -1;

static void request_stop(int signal_number) {
    (void)signal_number;
    stop_requested = 1;
    if (listen_fd_for_signal >= 0) {
        (void)close((int)listen_fd_for_signal);
        listen_fd_for_signal = -1;
    }
}

void sp_http_response_init(sp_http_response *response) {
    memset(response, 0, sizeof(*response));
    response->status = 200;
    response->content_type = "text/plain; charset=utf-8";
}

static sp_http_response owned_response(int status, const char *content_type,
                                       unsigned char *body, size_t body_len) {
    sp_http_response response;
    sp_http_response_init(&response);
    response.status = status;
    response.content_type = content_type;
    response.body = body;
    response.body_len = body_len;
    return response;
}

sp_http_response sp_http_text(int status, const char *text) {
    size_t len = strlen(text);
    unsigned char *body = malloc(len + 1U);
    if (body == NULL) {
        return owned_response(500, "text/plain; charset=utf-8", NULL, 0);
    }
    memcpy(body, text, len + 1U);
    return owned_response(status, "text/plain; charset=utf-8", body, len);
}

sp_http_response sp_http_json(int status, char *owned_json) {
    if (owned_json == NULL) {
        return sp_http_text(500, "Internal error");
    }
    return owned_response(status, "application/json; charset=utf-8",
                          (unsigned char *)owned_json, strlen(owned_json));
}

void sp_http_response_destroy(sp_http_response *response) {
    free(response->body);
    sp_http_response_init(response);
}

static const char *reason_phrase(int status) {
    switch (status) {
    case 200:
        return "OK";
    case 400:
        return "Bad Request";
    case 404:
        return "Not Found";
    case 405:
        return "Method Not Allowed";
    case 413:
        return "Content Too Large";
    case 431:
        return "Request Header Fields Too Large";
    case 500:
        return "Internal Server Error";
    case 501:
        return "Not Implemented";
    case 503:
        return "Service Unavailable";
    default:
        return "Response";
    }
}

static int send_all(int fd, const unsigned char *data, size_t len) {
    size_t sent = 0;
    while (sent < len) {
        ssize_t result = send(fd, data + sent, len - sent, MSG_NOSIGNAL);
        if (result < 0 && errno == EINTR) {
            continue;
        }
        if (result <= 0) {
            return -1;
        }
        sent += (size_t)result;
    }
    return 0;
}

static void send_response(int fd, const sp_http_response *response) {
    char headers[1024];
    int count = snprintf(
        headers, sizeof(headers),
        "HTTP/1.1 %d %s\r\n"
        "Content-Type: %s\r\n"
        "Content-Length: %zu\r\n"
        "Connection: close\r\n"
        "X-Content-Type-Options: nosniff\r\n"
        "%s"
        "\r\n",
        response->status, reason_phrase(response->status),
        response->content_type != NULL ? response->content_type
                                       : "application/octet-stream",
        response->body_len,
        response->no_cache ? "Cache-Control: no-cache\r\n" : "");
    if (count < 0 || (size_t)count >= sizeof(headers)) {
        return;
    }
    if (send_all(fd, (const unsigned char *)headers, (size_t)count) == 0 &&
        response->body_len > 0 && response->body != NULL) {
        (void)send_all(fd, response->body, response->body_len);
    }
}

static size_t header_end(const unsigned char *data, size_t len) {
    size_t index;
    for (index = 3; index < len; index++) {
        if (data[index - 3] == '\r' && data[index - 2] == '\n' &&
            data[index - 1] == '\r' && data[index] == '\n') {
            return index + 1U;
        }
    }
    return 0;
}

static int valid_token(const char *text) {
    const unsigned char *cursor = (const unsigned char *)text;
    if (*cursor == '\0') {
        return 0;
    }
    while (*cursor != '\0') {
        if (!(isalnum(*cursor) || strchr("!#$%&'*+-.^_`|~", *cursor) != NULL)) {
            return 0;
        }
        cursor++;
    }
    return 1;
}

static int parse_content_length(char *headers, size_t *body_len) {
    char *save = NULL;
    char *line;
    int found = 0;
    size_t expected = 0;
    for (line = strtok_r(headers, "\r\n", &save); line != NULL;
         line = strtok_r(NULL, "\r\n", &save)) {
        char *colon = strchr(line, ':');
        char *value;
        char *end;
        unsigned long long parsed;
        if (colon == NULL) {
            return 400;
        }
        *colon = '\0';
        if (!valid_token(line)) {
            return 400;
        }
        value = colon + 1;
        while (isspace((unsigned char)*value)) {
            value++;
        }
        if (strcasecmp(line, "Transfer-Encoding") == 0 && *value != '\0') {
            return 501;
        }
        if (strcasecmp(line, "Content-Length") != 0) {
            continue;
        }
        if (!isdigit((unsigned char)*value)) {
            return 400;
        }
        errno = 0;
        parsed = strtoull(value, &end, 10);
        while (isspace((unsigned char)*end)) {
            end++;
        }
        if (errno != 0 || *end != '\0' || parsed > SP_HTTP_MAX_BODY) {
            return parsed > SP_HTTP_MAX_BODY ? 413 : 400;
        }
        if (found && expected != (size_t)parsed) {
            return 400;
        }
        found = 1;
        expected = (size_t)parsed;
    }
    *body_len = expected;
    return 0;
}

static int read_request(int fd, sp_http_request *request) {
    unsigned char input[HEADER_LIMIT + 1U];
    size_t used = 0;
    size_t end = 0;
    size_t body_len = 0;
    size_t copied;
    char *headers;
    char *line_end;
    char *query;
    char version[16];
    char extra[2];

    int status;

    memset(request, 0, sizeof(*request));
    while ((end = header_end(input, used)) == 0) {
        ssize_t received;
        if (used >= HEADER_LIMIT) {
            return 431;
        }
        received = recv(fd, input + used, HEADER_LIMIT - used, 0);
        if (received < 0 && errno == EINTR) {
            continue;
        }
        if (received <= 0) {
            return 400;
        }
        used += (size_t)received;
    }
    if (end < 4 || end > HEADER_LIMIT || memchr(input, '\0', end) != NULL) {
        return 400;
    }
    input[end] = '\0';
    headers = (char *)input;
    line_end = strstr(headers, "\r\n");
    if (line_end == NULL) {
        return 400;
    }
    *line_end = '\0';
    if (sscanf(headers, "%15s %2047s %15s %1s", request->method,
               request->target, version, extra) != 3 ||
        (strcmp(version, "HTTP/1.1") != 0 && strcmp(version, "HTTP/1.0") != 0) ||
        !valid_token(request->method)) {
        return 400;
    }
    *line_end = '\r';
    status = parse_content_length(line_end + 2, &body_len);
    if (status != 0) {
        return status;
    }
    if (request->target[0] != '/') {
        return 400;
    }
    query = strchr(request->target, '?');
    if (query != NULL) {
        *query = '\0';
        request->query = query + 1;
    }
    if (strlen(request->target) >= sizeof(request->path)) {
        return 400;
    }
    strcpy(request->path, request->target);
    if (body_len > 0) {
        request->body = malloc(body_len);
        if (request->body == NULL) {
            return 500;
        }
    }
    request->body_len = body_len;
    copied = used - end;
    if (copied > body_len) {
        free(request->body);
        request->body = NULL;
        request->body_len = 0;
        return 400;
    }
    if (copied > 0) {
        memcpy(request->body, input + end, copied);
    }
    while (copied < body_len) {
        ssize_t received = recv(fd, request->body + copied, body_len - copied, 0);
        if (received < 0 && errno == EINTR) {
            continue;
        }
        if (received <= 0) {
            free(request->body);
            request->body = NULL;
            request->body_len = 0;
            return 400;
        }
        copied += (size_t)received;
    }
    return 0;
}

static void close_gracefully(int fd) {
    struct timeval drain_timeout;
    unsigned char drain[4096];
    size_t total = 0;

    /* close() con bytes pendientes en el socket puede emitir RST y hacer que
     * el cliente pierda una respuesta 413/431 ya enviada. Se cierra primero
     * nuestro lado de escritura y se drena una cantidad acotada de entrada. */
    (void)shutdown(fd, SHUT_WR);
    drain_timeout.tv_sec = 0;
    drain_timeout.tv_usec = 200000;
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &drain_timeout,
                     sizeof(drain_timeout));
    while (total < 256U * 1024U) {
        ssize_t received = recv(fd, drain, sizeof(drain), 0);
        if (received < 0 && errno == EINTR) {
            continue;
        }
        if (received <= 0) {
            break;
        }
        total += (size_t)received;
    }
    (void)close(fd);
}

static void handle_connection(int fd, const char *remote_ip,
                              sp_http_handler handler, void *context) {
    sp_http_request request;
    sp_http_response response;
    struct timeval timeout;
    int status;

    timeout.tv_sec = IO_TIMEOUT_SECONDS;
    timeout.tv_usec = 0;
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    (void)setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    status = read_request(fd, &request);
    if (status != 0) {
        response = sp_http_text(status, status == 413 ? "Request too large" :
                               status == 431 ? "Headers too large" :
                               status == 501 ? "Transfer encoding unsupported" :
                                               "Bad request");
        send_response(fd, &response);
        sp_http_response_destroy(&response);
        return;
    }
    strncpy(request.remote_ip, remote_ip, sizeof(request.remote_ip) - 1U);
    request.remote_ip[sizeof(request.remote_ip) - 1U] = '\0';
    response = handler(context, &request);
    send_response(fd, &response);
    sp_http_response_destroy(&response);
    free(request.body);
}

static void *worker_main(void *argument) {
    worker_pool *pool = argument;
    for (;;) {
        connection_job job;
        pthread_mutex_lock(&pool->mutex);
        while (pool->count == 0 && !pool->stopping) {
            (void)pthread_cond_wait(&pool->available, &pool->mutex);
        }
        if (pool->stopping && pool->count == 0) {
            pthread_mutex_unlock(&pool->mutex);
            break;
        }
        job = pool->queue[pool->head];
        pool->head = (pool->head + 1U) % QUEUE_LIMIT;
        pool->count--;
        (void)pthread_cond_signal(&pool->space);
        pthread_mutex_unlock(&pool->mutex);
        handle_connection(job.fd, job.remote_ip, pool->handler,
                          pool->handler_context);
        close_gracefully(job.fd);
    }
    return NULL;
}

static int split_address(const char *address, char *host, size_t host_size,
                         char *port, size_t port_size) {
    const char *port_start;
    size_t host_len;
    if (address[0] == '[') {
        const char *close = strchr(address, ']');
        if (close == NULL || close[1] != ':') {
            return -1;
        }
        host_len = (size_t)(close - address - 1);
        port_start = close + 2;
        if (host_len >= host_size) {
            return -1;
        }
        memcpy(host, address + 1, host_len);
        host[host_len] = '\0';
    } else {
        const char *colon = strrchr(address, ':');
        if (colon == NULL) {
            return -1;
        }
        host_len = (size_t)(colon - address);
        port_start = colon + 1;
        if (host_len >= host_size) {
            return -1;
        }
        memcpy(host, address, host_len);
        host[host_len] = '\0';
    }
    if (*port_start == '\0' || strlen(port_start) >= port_size) {
        return -1;
    }
    strcpy(port, port_start);
    return 0;
}

static int open_listener(const char *address, char **error) {
    char host[256];
    char port[16];
    struct addrinfo hints;
    struct addrinfo *addresses = NULL;
    struct addrinfo *current;
    int fd = -1;
    int gai_error;

    if (split_address(address, host, sizeof(host), port, sizeof(port)) != 0) {
        sp_set_error(error, "dirección inválida: %s (usa HOST:PUERTO)", address);
        return -1;
    }
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;
    gai_error = getaddrinfo(*host != '\0' ? host : NULL, port, &hints, &addresses);
    if (gai_error != 0) {
        sp_set_error(error, "getaddrinfo: %s", gai_strerror(gai_error));
        return -1;
    }
    for (current = addresses; current != NULL; current = current->ai_next) {
        int reuse = 1;
        fd = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (fd < 0) {
            continue;
        }
        (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
        if (bind(fd, current->ai_addr, current->ai_addrlen) == 0 &&
            listen(fd, (int)QUEUE_LIMIT) == 0) {
            break;
        }
        (void)close(fd);
        fd = -1;
    }
    if (fd < 0) {
        sp_set_error(error, "no se pudo escuchar en %s: %s", address,
                     strerror(errno));
    }
    freeaddrinfo(addresses);
    return fd;
}

static void pool_destroy(worker_pool *pool) {
    size_t index;
    pthread_mutex_lock(&pool->mutex);
    pool->stopping = 1;
    (void)pthread_cond_broadcast(&pool->available);
    (void)pthread_cond_broadcast(&pool->space);
    pthread_mutex_unlock(&pool->mutex);
    for (index = 0; index < pool->thread_count; index++) {
        (void)pthread_join(pool->threads[index], NULL);
    }
    while (pool->count > 0) {
        int fd = pool->queue[pool->head].fd;
        pool->head = (pool->head + 1U) % QUEUE_LIMIT;
        pool->count--;
        (void)close(fd);
    }
    free(pool->threads);
    (void)pthread_cond_destroy(&pool->available);
    (void)pthread_cond_destroy(&pool->space);
    (void)pthread_mutex_destroy(&pool->mutex);
}

int sp_http_serve(const char *address, unsigned int workers,
                  sp_http_handler handler, void *context, char **error) {
    worker_pool pool;
    struct sigaction action;
    unsigned int index;
    int listener;
    int mutex_ready;
    int available_ready;
    int space_ready;
    int result = -1;

    memset(&pool, 0, sizeof(pool));
    pool.handler = handler;
    pool.handler_context = context;
    if (workers == 0) {
        workers = 4;
    }
    if (workers > 64) {
        workers = 64;
    }
    listener = open_listener(address, error);
    if (listener < 0) {
        return -1;
    }
    mutex_ready = pthread_mutex_init(&pool.mutex, NULL) == 0;
    available_ready =
        mutex_ready && pthread_cond_init(&pool.available, NULL) == 0;
    space_ready = available_ready && pthread_cond_init(&pool.space, NULL) == 0;
    if (!space_ready) {
        if (available_ready) {
            (void)pthread_cond_destroy(&pool.available);
        }
        if (mutex_ready) {
            (void)pthread_mutex_destroy(&pool.mutex);
        }
        sp_set_error(error, "no se pudieron iniciar las primitivas de hilos");
        (void)close(listener);
        return -1;
    }
    pool.threads = calloc(workers, sizeof(*pool.threads));
    if (pool.threads == NULL) {
        sp_set_error(error, "memoria insuficiente para workers");
        pool_destroy(&pool);
        (void)close(listener);
        return -1;
    }
    memset(&action, 0, sizeof(action));
    action.sa_handler = request_stop;
    (void)sigemptyset(&action.sa_mask);
    (void)sigaction(SIGINT, &action, NULL);
    (void)sigaction(SIGTERM, &action, NULL);
    (void)signal(SIGPIPE, SIG_IGN);
    stop_requested = 0;
    listen_fd_for_signal = listener;

    for (index = 0; index < workers; index++) {
        if (pthread_create(&pool.threads[index], NULL, worker_main, &pool) != 0) {
            sp_set_error(error, "no se pudo crear worker %u", index);
            pool.thread_count = index;
            listen_fd_for_signal = -1;
            pool_destroy(&pool);
            (void)close(listener);
            return -1;
        }
        pool.thread_count++;
    }
    fprintf(stderr, "Backend C escuchando en %s (%u workers)\n", address,
            pool.thread_count);

    while (!stop_requested) {
        struct sockaddr_storage remote;
        socklen_t remote_len = sizeof(remote);
        connection_job job;
        job.fd = accept(listener, (struct sockaddr *)&remote, &remote_len);
        if (job.fd < 0) {
            if (errno == EINTR) {
                continue;
            }
            if (stop_requested || errno == EBADF || errno == EINVAL) {
                break;
            }
            continue;
        }
        if (getnameinfo((struct sockaddr *)&remote, remote_len, job.remote_ip,
                        sizeof(job.remote_ip), NULL, 0, NI_NUMERICHOST) != 0) {
            strcpy(job.remote_ip, "unknown");
        }
        pthread_mutex_lock(&pool.mutex);
        while (pool.count == QUEUE_LIMIT && !pool.stopping) {
            (void)pthread_cond_wait(&pool.space, &pool.mutex);
        }
        if (pool.stopping) {
            pthread_mutex_unlock(&pool.mutex);
            (void)close(job.fd);
            break;
        }
        pool.queue[pool.tail] = job;
        pool.tail = (pool.tail + 1U) % QUEUE_LIMIT;
        pool.count++;
        (void)pthread_cond_signal(&pool.available);
        pthread_mutex_unlock(&pool.mutex);
    }
    result = 0;
    if (listen_fd_for_signal >= 0) {
        (void)close(listener);
        listen_fd_for_signal = -1;
    }
    pool_destroy(&pool);
    return result;
}
