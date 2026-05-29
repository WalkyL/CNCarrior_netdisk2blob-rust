/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#include "relay_lite_poc.h"

#include <stdio.h>
#include <string.h>

typedef struct MemoryProvider {
    uint8_t object[CCBG_RELAY_LITE_MAX_OBJECT_BYTES];
    size_t len;
} MemoryProvider;

typedef struct Body {
    const uint8_t *data;
    size_t len;
    size_t offset;
} Body;

static int memory_put(
    void *ctx,
    const char *bucket,
    const char *key,
    const uint8_t *chunk,
    size_t chunk_len,
    int final_chunk) {
    MemoryProvider *provider = (MemoryProvider *)ctx;
    (void)bucket;
    (void)key;
    (void)final_chunk;
    if (provider->len + chunk_len > sizeof(provider->object)) {
        return -1;
    }
    memcpy(provider->object + provider->len, chunk, chunk_len);
    provider->len += chunk_len;
    return 0;
}

static int memory_get(
    void *ctx,
    const char *bucket,
    const char *key,
    uint32_t offset,
    uint8_t *chunk,
    size_t capacity,
    size_t *out_len,
    int *out_final_chunk) {
    MemoryProvider *provider = (MemoryProvider *)ctx;
    size_t remaining;
    size_t take;
    (void)bucket;
    (void)key;
    if (offset > provider->len) {
        return -1;
    }
    remaining = provider->len - offset;
    take = remaining < capacity ? remaining : capacity;
    if (take > 0u) {
        memcpy(chunk, provider->object + offset, take);
    }
    *out_len = take;
    *out_final_chunk = (offset + take) >= provider->len ? 1 : 0;
    return 0;
}

static size_t read_body(void *ctx, uint8_t *buffer, size_t capacity) {
    Body *body = (Body *)ctx;
    size_t remaining = body->len - body->offset;
    size_t take = remaining < capacity ? remaining : capacity;
    if (take > 0u) {
        memcpy(buffer, body->data + body->offset, take);
        body->offset += take;
    }
    return take;
}

static size_t write_body(void *ctx, const uint8_t *buffer, size_t len) {
    size_t *total = (size_t *)ctx;
    (void)buffer;
    *total += len;
    return len;
}

int main(void) {
    static const uint8_t sample[] = "relay-lite-poc";
    MemoryProvider memory = {{0}, 0u};
    CcbgRelayLite relay;
    CcbgRelayLiteProvider provider = {memory_put, memory_get, &memory};
    Body upload = {sample, sizeof(sample) - 1u, 0u};
    size_t downloaded = 0u;
    if (ccbg_relay_lite_init(&relay, &provider) != CCBG_RELAY_LITE_OK) {
        return 1;
    }
    if (ccbg_relay_lite_put(&relay, "root", "relay/demo.txt", (uint32_t)upload.len, read_body, &upload) != CCBG_RELAY_LITE_OK) {
        return 2;
    }
    if (ccbg_relay_lite_get(&relay, "root", "relay/demo.txt", write_body, &downloaded) != CCBG_RELAY_LITE_OK) {
        return 3;
    }
    if (downloaded != sizeof(sample) - 1u) {
        return 4;
    }
    printf("relay-lite PoC transferred %u bytes\n", (unsigned)downloaded);
    return 0;
}
