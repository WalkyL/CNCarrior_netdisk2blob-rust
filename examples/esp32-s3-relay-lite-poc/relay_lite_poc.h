/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#ifndef CCBG_RELAY_LITE_POC_H
#define CCBG_RELAY_LITE_POC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CCBG_RELAY_LITE_CHUNK_BYTES 1024u
#define CCBG_RELAY_LITE_MAX_OBJECT_BYTES (64u * 1024u)

typedef enum CcbgRelayLiteStatus {
    CCBG_RELAY_LITE_OK = 0,
    CCBG_RELAY_LITE_ERR_INVALID_ARG = -1,
    CCBG_RELAY_LITE_ERR_BUSY = -2,
    CCBG_RELAY_LITE_ERR_LIMIT = -3,
    CCBG_RELAY_LITE_ERR_PROVIDER = -4
} CcbgRelayLiteStatus;

typedef size_t (*CcbgRelayLiteReadFn)(void *ctx, uint8_t *buffer, size_t capacity);
typedef size_t (*CcbgRelayLiteWriteFn)(void *ctx, const uint8_t *buffer, size_t len);

typedef int (*CcbgRelayLiteProviderPutFn)(
    void *ctx,
    const char *bucket,
    const char *key,
    const uint8_t *chunk,
    size_t chunk_len,
    int final_chunk);

typedef int (*CcbgRelayLiteProviderGetFn)(
    void *ctx,
    const char *bucket,
    const char *key,
    uint32_t offset,
    uint8_t *chunk,
    size_t capacity,
    size_t *out_len,
    int *out_final_chunk);

typedef struct CcbgRelayLiteProvider {
    CcbgRelayLiteProviderPutFn put_chunk;
    CcbgRelayLiteProviderGetFn get_chunk;
    void *ctx;
} CcbgRelayLiteProvider;

typedef struct CcbgRelayLite {
    CcbgRelayLiteProvider provider;
    uint8_t busy;
} CcbgRelayLite;

CcbgRelayLiteStatus ccbg_relay_lite_init(
    CcbgRelayLite *relay,
    const CcbgRelayLiteProvider *provider);

CcbgRelayLiteStatus ccbg_relay_lite_put(
    CcbgRelayLite *relay,
    const char *bucket,
    const char *key,
    uint32_t content_length,
    CcbgRelayLiteReadFn read_body,
    void *body_ctx);

CcbgRelayLiteStatus ccbg_relay_lite_get(
    CcbgRelayLite *relay,
    const char *bucket,
    const char *key,
    CcbgRelayLiteWriteFn write_body,
    void *body_ctx);

#ifdef __cplusplus
}
#endif

#endif
