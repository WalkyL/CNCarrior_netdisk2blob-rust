/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#include "relay_lite_poc.h"

#include <string.h>

static int invalid_name(const char *value) {
    return value == 0 || value[0] == '\0';
}

CcbgRelayLiteStatus ccbg_relay_lite_init(
    CcbgRelayLite *relay,
    const CcbgRelayLiteProvider *provider) {
    if (relay == 0 || provider == 0 || provider->put_chunk == 0 || provider->get_chunk == 0) {
        return CCBG_RELAY_LITE_ERR_INVALID_ARG;
    }
    memset(relay, 0, sizeof(*relay));
    relay->provider = *provider;
    return CCBG_RELAY_LITE_OK;
}

CcbgRelayLiteStatus ccbg_relay_lite_put(
    CcbgRelayLite *relay,
    const char *bucket,
    const char *key,
    uint32_t content_length,
    CcbgRelayLiteReadFn read_body,
    void *body_ctx) {
    uint8_t chunk[CCBG_RELAY_LITE_CHUNK_BYTES];
    uint32_t sent = 0u;
    if (relay == 0 || invalid_name(bucket) || invalid_name(key) || read_body == 0) {
        return CCBG_RELAY_LITE_ERR_INVALID_ARG;
    }
    if (relay->busy != 0u) {
        return CCBG_RELAY_LITE_ERR_BUSY;
    }
    if (content_length > CCBG_RELAY_LITE_MAX_OBJECT_BYTES) {
        return CCBG_RELAY_LITE_ERR_LIMIT;
    }
    relay->busy = 1u;
    while (sent < content_length) {
        size_t expected = content_length - sent;
        size_t got;
        int final_chunk;
        if (expected > sizeof(chunk)) {
            expected = sizeof(chunk);
        }
        got = read_body(body_ctx, chunk, expected);
        if (got == 0u || got > expected) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_PROVIDER;
        }
        sent += (uint32_t)got;
        final_chunk = sent == content_length ? 1 : 0;
        if (relay->provider.put_chunk(relay->provider.ctx, bucket, key, chunk, got, final_chunk) != 0) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_PROVIDER;
        }
    }
    relay->busy = 0u;
    return CCBG_RELAY_LITE_OK;
}

CcbgRelayLiteStatus ccbg_relay_lite_get(
    CcbgRelayLite *relay,
    const char *bucket,
    const char *key,
    CcbgRelayLiteWriteFn write_body,
    void *body_ctx) {
    uint8_t chunk[CCBG_RELAY_LITE_CHUNK_BYTES];
    uint32_t offset = 0u;
    int final_chunk = 0;
    if (relay == 0 || invalid_name(bucket) || invalid_name(key) || write_body == 0) {
        return CCBG_RELAY_LITE_ERR_INVALID_ARG;
    }
    if (relay->busy != 0u) {
        return CCBG_RELAY_LITE_ERR_BUSY;
    }
    relay->busy = 1u;
    while (final_chunk == 0) {
        size_t got = 0u;
        if (offset > CCBG_RELAY_LITE_MAX_OBJECT_BYTES) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_LIMIT;
        }
        if (relay->provider.get_chunk(
                relay->provider.ctx,
                bucket,
                key,
                offset,
                chunk,
                sizeof(chunk),
                &got,
                &final_chunk) != 0) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_PROVIDER;
        }
        if (got > sizeof(chunk) || offset + (uint32_t)got > CCBG_RELAY_LITE_MAX_OBJECT_BYTES) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_LIMIT;
        }
        if (got > 0u && write_body(body_ctx, chunk, got) != got) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_PROVIDER;
        }
        offset += (uint32_t)got;
        if (got == 0u && final_chunk == 0) {
            relay->busy = 0u;
            return CCBG_RELAY_LITE_ERR_PROVIDER;
        }
    }
    relay->busy = 0u;
    return CCBG_RELAY_LITE_OK;
}
