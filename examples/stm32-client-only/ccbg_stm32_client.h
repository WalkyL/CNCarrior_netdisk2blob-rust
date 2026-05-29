/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#ifndef CCBG_STM32_CLIENT_H
#define CCBG_STM32_CLIENT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CCBG_STM32_MAX_HOST_LEN 96u
#define CCBG_STM32_MAX_REGION_LEN 32u
#define CCBG_STM32_MAX_ACCESS_KEY_LEN 64u
#define CCBG_STM32_MAX_SECRET_KEY_LEN 96u
#define CCBG_STM32_MAX_BUCKET_LEN 64u
#define CCBG_STM32_MAX_KEY_LEN 160u
#define CCBG_STM32_MAX_URI_LEN 256u
#define CCBG_STM32_MAX_HEADER_VALUE_LEN 256u
#define CCBG_STM32_IO_CHUNK_BYTES 1024u
#define CCBG_STM32_MAX_OBJECT_BYTES (32u * 1024u)

typedef enum CcbgStm32Status {
    CCBG_STM32_OK = 0,
    CCBG_STM32_ERR_INVALID_ARG = -1,
    CCBG_STM32_ERR_BUFFER_TOO_SMALL = -2,
    CCBG_STM32_ERR_CRYPTO = -3,
    CCBG_STM32_ERR_TIME = -4,
    CCBG_STM32_ERR_HTTP = -5,
    CCBG_STM32_ERR_REMOTE = -6,
    CCBG_STM32_ERR_RETRY_EXHAUSTED = -7
} CcbgStm32Status;

typedef enum CcbgStm32Method {
    CCBG_STM32_HTTP_HEAD = 0,
    CCBG_STM32_HTTP_GET = 1,
    CCBG_STM32_HTTP_PUT = 2
} CcbgStm32Method;

typedef struct CcbgStm32Header {
    const char *name;
    const char *value;
} CcbgStm32Header;

typedef size_t (*CcbgStm32ReadBodyFn)(void *ctx, uint8_t *buffer, size_t capacity);
typedef size_t (*CcbgStm32WriteBodyFn)(void *ctx, const uint8_t *data, size_t len);

typedef struct CcbgStm32HttpRequest {
    CcbgStm32Method method;
    const char *uri;
    const CcbgStm32Header *headers;
    size_t header_count;
    uint32_t content_length;
    CcbgStm32ReadBodyFn read_body;
    void *body_ctx;
    uint32_t timeout_ms;
} CcbgStm32HttpRequest;

typedef struct CcbgStm32HttpResponse {
    int status_code;
    CcbgStm32WriteBodyFn write_body;
    void *body_ctx;
} CcbgStm32HttpResponse;

typedef int (*CcbgStm32Sha256HexFn)(
    void *ctx,
    const uint8_t *data,
    size_t len,
    char out_hex65[65]);

typedef int (*CcbgStm32HmacSha256Fn)(
    void *ctx,
    const uint8_t *key,
    size_t key_len,
    const uint8_t *data,
    size_t data_len,
    uint8_t out32[32]);

typedef int (*CcbgStm32UtcNowFn)(
    void *ctx,
    char out_amz_date17[17],
    char out_date9[9]);

typedef int (*CcbgStm32HttpRequestFn)(
    void *ctx,
    const CcbgStm32HttpRequest *request,
    CcbgStm32HttpResponse *response);

typedef struct CcbgStm32Platform {
    CcbgStm32Sha256HexFn sha256_hex;
    CcbgStm32HmacSha256Fn hmac_sha256;
    CcbgStm32UtcNowFn utc_now;
    CcbgStm32HttpRequestFn http_request;
    void *crypto_ctx;
    void *time_ctx;
    void *http_ctx;
} CcbgStm32Platform;

typedef struct CcbgStm32Config {
    const char *host;
    const char *region;
    const char *access_key;
    const char *secret_key;
    uint32_t request_timeout_ms;
    uint8_t max_attempts;
    uint32_t retry_backoff_ms;
} CcbgStm32Config;

typedef struct CcbgStm32Client {
    CcbgStm32Config config;
    CcbgStm32Platform platform;
} CcbgStm32Client;

CcbgStm32Status ccbg_stm32_client_init(
    CcbgStm32Client *client,
    const CcbgStm32Config *config,
    const CcbgStm32Platform *platform);

CcbgStm32Status ccbg_stm32_head_object(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key);

CcbgStm32Status ccbg_stm32_get_object(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key,
    CcbgStm32WriteBodyFn write_body,
    void *body_ctx);

CcbgStm32Status ccbg_stm32_put_object_stream(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key,
    uint32_t content_length,
    CcbgStm32ReadBodyFn read_body,
    void *body_ctx);

#ifdef __cplusplus
}
#endif

#endif
