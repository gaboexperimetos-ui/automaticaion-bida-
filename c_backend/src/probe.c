#include "probe.h"

#include "util.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <poll.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#define PROBE_RESPONSE_LIMIT 4096U

static int connect_timeout(const struct addrinfo *address, int timeout_ms) {
    int fd = socket(address->ai_family, address->ai_socktype,
                    address->ai_protocol);
    int flags;
    int code;
    struct pollfd descriptor;
    int socket_error = 0;
    socklen_t error_size = sizeof(socket_error);

    if (fd < 0) {
        return -1;
    }
    flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) != 0) {
        (void)close(fd);
        return -1;
    }
    code = connect(fd, address->ai_addr, address->ai_addrlen);
    if (code != 0 && errno != EINPROGRESS) {
        (void)close(fd);
        return -1;
    }
    if (code != 0) {
        descriptor.fd = fd;
        descriptor.events = POLLOUT;
        descriptor.revents = 0;
        do {
            code = poll(&descriptor, 1, timeout_ms);
        } while (code < 0 && errno == EINTR);
        if (code <= 0 ||
            getsockopt(fd, SOL_SOCKET, SO_ERROR, &socket_error, &error_size) !=
                0 ||
            socket_error != 0) {
            (void)close(fd);
            return -1;
        }
    }
    if (fcntl(fd, F_SETFL, flags) != 0) {
        (void)close(fd);
        return -1;
    }
    return fd;
}

static bool send_request(int fd, const char *host_header, const char *path,
                         uint64_t timeout_ms) {
    static const char body[] =
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{"
        "\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{"
        "\"name\":\"study-platform-health\",\"version\":\"0.1.0\"}}}";
    sp_buffer request;
    unsigned char response[PROBE_RESPONSE_LIMIT + 1U];
    size_t used = 0;
    size_t sent = 0;
    struct timeval timeout;
    int status = 0;
    bool ok = false;

    timeout.tv_sec = (time_t)(timeout_ms / 1000U);
    timeout.tv_usec = (suseconds_t)((timeout_ms % 1000U) * 1000U);
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    (void)setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    sp_buffer_init(&request);
    if (sp_buffer_appendf(&request,
                          "POST %s HTTP/1.1\r\n"
                          "Host: %s\r\n"
                          "Content-Type: application/json\r\n"
                          "Accept: application/json, text/event-stream\r\n"
                          "Content-Length: %zu\r\n"
                          "Connection: close\r\n\r\n",
                          path, host_header, sizeof(body) - 1U) != 0 ||
        sp_buffer_append_n(&request, body, sizeof(body) - 1U) != 0) {
        sp_buffer_destroy(&request);
        return false;
    }
    while (sent < request.len) {
        ssize_t result = send(fd, request.data + sent, request.len - sent,
                              MSG_NOSIGNAL);
        if (result < 0 && errno == EINTR) {
            continue;
        }
        if (result <= 0) {
            sp_buffer_destroy(&request);
            return false;
        }
        sent += (size_t)result;
    }
    sp_buffer_destroy(&request);

    while (used < PROBE_RESPONSE_LIMIT) {
        ssize_t result = recv(fd, response + used, PROBE_RESPONSE_LIMIT - used, 0);
        char *line_end;
        if (result < 0 && errno == EINTR) {
            continue;
        }
        if (result <= 0) {
            break;
        }
        used += (size_t)result;
        response[used] = '\0';
        line_end = strstr((char *)response, "\r\n");
        if (line_end != NULL) {
            *line_end = '\0';
            if (sscanf((char *)response, "HTTP/%*s %d", &status) == 1) {
                ok = status >= 200 && status < 300;
            }
            break;
        }
    }
    return ok;
}

bool sp_probe_mcp_http(const char *url, uint64_t timeout_ms) {
    const char *authority;
    const char *authority_end;
    const char *path;
    const char *port_start = NULL;
    char host[256];
    char port[16] = "80";
    char host_header[320];
    struct addrinfo hints;
    struct addrinfo *addresses = NULL;
    struct addrinfo *current;
    int fd = -1;
    bool result = false;
    int timeout = timeout_ms > 2500U ? 2500 : (int)timeout_ms;

    if (url == NULL || strncmp(url, "http://", 7U) != 0 || timeout == 0) {
        return false;
    }
    authority = url + 7;
    authority_end = authority + strcspn(authority, "/?#");
    path = *authority_end == '/' ? authority_end : "/";
    if (memchr(authority, '@', (size_t)(authority_end - authority)) != NULL) {
        return false;
    }
    if (*authority == '[') {
        const char *close = memchr(authority, ']',
                                   (size_t)(authority_end - authority));
        size_t host_len;
        if (close == NULL) {
            return false;
        }
        host_len = (size_t)(close - authority - 1);
        if (host_len == 0 || host_len >= sizeof(host)) {
            return false;
        }
        memcpy(host, authority + 1, host_len);
        host[host_len] = '\0';
        if (close + 1 < authority_end) {
            if (close[1] != ':') {
                return false;
            }
            port_start = close + 2;
        }
    } else {
        const char *colon = NULL;
        const char *cursor;
        size_t host_len;
        for (cursor = authority; cursor < authority_end; cursor++) {
            if (*cursor == ':') {
                colon = cursor;
            }
        }
        host_len = colon != NULL ? (size_t)(colon - authority)
                                 : (size_t)(authority_end - authority);
        if (host_len == 0 || host_len >= sizeof(host)) {
            return false;
        }
        memcpy(host, authority, host_len);
        host[host_len] = '\0';
        if (colon != NULL) {
            port_start = colon + 1;
        }
    }
    if (port_start != NULL) {
        size_t port_len = (size_t)(authority_end - port_start);
        if (port_len == 0 || port_len >= sizeof(port)) {
            return false;
        }
        memcpy(port, port_start, port_len);
        port[port_len] = '\0';
    }
    if (snprintf(host_header, sizeof(host_header), "%s:%s", host, port) < 0) {
        return false;
    }

    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(host, port, &hints, &addresses) != 0) {
        return false;
    }
    for (current = addresses; current != NULL; current = current->ai_next) {
        fd = connect_timeout(current, timeout);
        if (fd >= 0) {
            break;
        }
    }
    freeaddrinfo(addresses);
    if (fd >= 0) {
        result = send_request(fd, host_header, path, (uint64_t)timeout);
        (void)close(fd);
    }
    return result;
}
