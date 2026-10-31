#ifndef SP_PROBE_H
#define SP_PROBE_H

#include <stdbool.h>
#include <stdint.h>

bool sp_probe_mcp_http(const char *url, uint64_t timeout_ms);

#endif
