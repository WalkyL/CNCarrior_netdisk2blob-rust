/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#include "ccbg_stm32_client.h"

#include <stdio.h>
#include <string.h>

static int is_empty(const char *value) {
    return value == 0 || value[0] == '\0';
}

static size_t bounded_strlen(const char *value, size_t max_len) {
    size_t len = 0;
    if (value == 0) {
        return 0;
    }
    while (len < max_len && value[len] != '\0') {
        len++;
    }
    return len;
}

static CcbgStm32Status validate_config(const CcbgStm32Config *config) {
    if (config == 0 || is_empty(config->host) || is_empty(config->region) ||
        is_empty(config->access_key) || is_empty(config->secret_key)) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    if (bounded_strlen(config->host, CCBG_STM32_MAX_HOST_LEN + 1u) > CCBG_STM32_MAX_HOST_LEN ||
        bounded_strlen(config->region, CCBG_STM32_MAX_REGION_LEN + 1u) > CCBG_STM32_MAX_REGION_LEN ||
        bounded_strlen(config->access_key, CCBG_STM32_MAX_ACCESS_KEY_LEN + 1u) > CCBG_STM32_MAX_ACCESS_KEY_LEN ||
        bounded_strlen(config->secret_key, CCBG_STM32_MAX_SECRET_KEY_LEN + 1u) > CCBG_STM32_MAX_SECRET_KEY_LEN) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    return CCBG_STM32_OK;
}

static CcbgStm32Status validate_platform(const CcbgStm32Platform *platform) {
    if (platform == 0 || platform->sha256_hex == 0 || platform->hmac_sha256 == 0 ||
        platform->utc_now == 0 || platform->http_request == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    return CCBG_STM32_OK;
}

static const char *method_name(CcbgStm32Method method) {
    switch (method) {
    case CCBG_STM32_HTTP_HEAD:
        return "HEAD";
    case CCBG_STM32_HTTP_GET:
        return "GET";
    case CCBG_STM32_HTTP_PUT:
        return "PUT";
    default:
        return "GET";
    }
}

static int is_unreserved(char ch) {
    return (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') ||
           (ch >= '0' && ch <= '9') || ch == '-' || ch == '_' ||
           ch == '.' || ch == '~';
}

static CcbgStm32Status append_uri_part(char *out, size_t out_len, size_t *used, const char *value) {
    static const char hex[] = "0123456789ABCDEF";
    size_t i = 0;
    if (out == 0 || used == 0 || value == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    while (value[i] != '\0') {
        unsigned char ch = (unsigned char)value[i++];
        if (is_unreserved((char)ch)) {
            if (*used + 1u >= out_len) {
                return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
            }
            out[*used] = (char)ch;
            *used += 1u;
        } else {
            if (*used + 3u >= out_len) {
                return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
            }
            out[*used] = '%';
            out[*used + 1u] = hex[(ch >> 4u) & 0x0fu];
            out[*used + 2u] = hex[ch & 0x0fu];
            *used += 3u;
        }
    }
    out[*used] = '\0';
    return CCBG_STM32_OK;
}

static CcbgStm32Status build_uri(char *out, size_t out_len, const char *bucket, const char *key) {
    size_t used = 0;
    if (out_len < 2u || is_empty(bucket) || is_empty(key)) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    if (bounded_strlen(bucket, CCBG_STM32_MAX_BUCKET_LEN + 1u) > CCBG_STM32_MAX_BUCKET_LEN ||
        bounded_strlen(key, CCBG_STM32_MAX_KEY_LEN + 1u) > CCBG_STM32_MAX_KEY_LEN) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    out[used++] = '/';
    out[used] = '\0';
    if (append_uri_part(out, out_len, &used, bucket) != CCBG_STM32_OK) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    if (used + 1u >= out_len) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    out[used++] = '/';
    out[used] = '\0';
    return append_uri_part(out, out_len, &used, key);
}

static void bytes_to_hex(const uint8_t *bytes, size_t len, char *out) {
    static const char hex[] = "0123456789abcdef";
    size_t i = 0;
    for (i = 0; i < len; i++) {
        out[i * 2u] = hex[(bytes[i] >> 4u) & 0x0fu];
        out[i * 2u + 1u] = hex[bytes[i] & 0x0fu];
    }
    out[len * 2u] = '\0';
}

static CcbgStm32Status hmac(
    const CcbgStm32Client *client,
    const uint8_t *key,
    size_t key_len,
    const char *data,
    uint8_t out32[32]) {
    if (client->platform.hmac_sha256(
            client->platform.crypto_ctx,
            key,
            key_len,
            (const uint8_t *)data,
            strlen(data),
            out32) != 0) {
        return CCBG_STM32_ERR_CRYPTO;
    }
    return CCBG_STM32_OK;
}

static CcbgStm32Status build_authorization(
    const CcbgStm32Client *client,
    CcbgStm32Method method,
    const char *uri,
    const char *amz_date,
    const char *date,
    const char *payload_hash,
    char out_auth[CCBG_STM32_MAX_HEADER_VALUE_LEN]) {
    char canonical_request[768];
    char canonical_hash[65];
    char string_to_sign[256];
    char aws4_secret[128];
    uint8_t k_date[32];
    uint8_t k_region[32];
    uint8_t k_service[32];
    uint8_t k_signing[32];
    uint8_t signature_raw[32];
    char signature_hex[65];
    int written;

    written = snprintf(
        canonical_request,
        sizeof(canonical_request),
        "%s\n%s\n\nhost:%s\nx-amz-content-sha256:%s\nx-amz-date:%s\n\nhost;x-amz-content-sha256;x-amz-date\n%s",
        method_name(method),
        uri,
        client->config.host,
        payload_hash,
        amz_date,
        payload_hash);
    if (written < 0 || (size_t)written >= sizeof(canonical_request)) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    if (client->platform.sha256_hex(
            client->platform.crypto_ctx,
            (const uint8_t *)canonical_request,
            strlen(canonical_request),
            canonical_hash) != 0) {
        return CCBG_STM32_ERR_CRYPTO;
    }
    written = snprintf(
        string_to_sign,
        sizeof(string_to_sign),
        "AWS4-HMAC-SHA256\n%s\n%s/%s/s3/aws4_request\n%s",
        amz_date,
        date,
        client->config.region,
        canonical_hash);
    if (written < 0 || (size_t)written >= sizeof(string_to_sign)) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    written = snprintf(aws4_secret, sizeof(aws4_secret), "AWS4%s", client->config.secret_key);
    if (written < 0 || (size_t)written >= sizeof(aws4_secret)) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    if (hmac(client, (const uint8_t *)aws4_secret, strlen(aws4_secret), date, k_date) != CCBG_STM32_OK ||
        hmac(client, k_date, sizeof(k_date), client->config.region, k_region) != CCBG_STM32_OK ||
        hmac(client, k_region, sizeof(k_region), "s3", k_service) != CCBG_STM32_OK ||
        hmac(client, k_service, sizeof(k_service), "aws4_request", k_signing) != CCBG_STM32_OK ||
        hmac(client, k_signing, sizeof(k_signing), string_to_sign, signature_raw) != CCBG_STM32_OK) {
        return CCBG_STM32_ERR_CRYPTO;
    }
    bytes_to_hex(signature_raw, sizeof(signature_raw), signature_hex);
    written = snprintf(
        out_auth,
        CCBG_STM32_MAX_HEADER_VALUE_LEN,
        "AWS4-HMAC-SHA256 Credential=%s/%s/%s/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=%s",
        client->config.access_key,
        date,
        client->config.region,
        signature_hex);
    if (written < 0 || (size_t)written >= CCBG_STM32_MAX_HEADER_VALUE_LEN) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    return CCBG_STM32_OK;
}

static CcbgStm32Status send_once(
    CcbgStm32Client *client,
    CcbgStm32Method method,
    const char *uri,
    uint32_t content_length,
    CcbgStm32ReadBodyFn read_body,
    void *read_ctx,
    CcbgStm32WriteBodyFn write_body,
    void *write_ctx) {
    char amz_date[17];
    char date[9];
    char auth[CCBG_STM32_MAX_HEADER_VALUE_LEN];
    const char *payload_hash = "UNSIGNED-PAYLOAD";
    CcbgStm32Header headers[4];
    CcbgStm32HttpRequest request;
    CcbgStm32HttpResponse response;
    CcbgStm32Status status;

    if (client->platform.utc_now(client->platform.time_ctx, amz_date, date) != 0) {
        return CCBG_STM32_ERR_TIME;
    }
    status = build_authorization(client, method, uri, amz_date, date, payload_hash, auth);
    if (status != CCBG_STM32_OK) {
        return status;
    }

    headers[0].name = "host";
    headers[0].value = client->config.host;
    headers[1].name = "x-amz-content-sha256";
    headers[1].value = payload_hash;
    headers[2].name = "x-amz-date";
    headers[2].value = amz_date;
    headers[3].name = "Authorization";
    headers[3].value = auth;

    request.method = method;
    request.uri = uri;
    request.headers = headers;
    request.header_count = 4u;
    request.content_length = content_length;
    request.read_body = read_body;
    request.body_ctx = read_ctx;
    request.timeout_ms = client->config.request_timeout_ms;
    response.status_code = 0;
    response.write_body = write_body;
    response.body_ctx = write_ctx;

    if (client->platform.http_request(client->platform.http_ctx, &request, &response) != 0) {
        return CCBG_STM32_ERR_HTTP;
    }
    if (response.status_code < 200 || response.status_code >= 300) {
        return CCBG_STM32_ERR_REMOTE;
    }
    return CCBG_STM32_OK;
}

static CcbgStm32Status request_with_retry(
    CcbgStm32Client *client,
    CcbgStm32Method method,
    const char *uri,
    uint32_t content_length,
    CcbgStm32ReadBodyFn read_body,
    void *read_ctx,
    CcbgStm32WriteBodyFn write_body,
    void *write_ctx) {
    uint8_t attempt = 0;
    uint8_t max_attempts = client->config.max_attempts == 0u ? 1u : client->config.max_attempts;
    CcbgStm32Status last = CCBG_STM32_ERR_RETRY_EXHAUSTED;
    for (attempt = 0; attempt < max_attempts; attempt++) {
        last = send_once(client, method, uri, content_length, read_body, read_ctx, write_body, write_ctx);
        if (last == CCBG_STM32_OK || last == CCBG_STM32_ERR_REMOTE) {
            return last;
        }
    }
    return last;
}

CcbgStm32Status ccbg_stm32_client_init(
    CcbgStm32Client *client,
    const CcbgStm32Config *config,
    const CcbgStm32Platform *platform) {
    CcbgStm32Status status;
    if (client == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    status = validate_config(config);
    if (status != CCBG_STM32_OK) {
        return status;
    }
    status = validate_platform(platform);
    if (status != CCBG_STM32_OK) {
        return status;
    }
    memset(client, 0, sizeof(*client));
    client->config = *config;
    client->platform = *platform;
    if (client->config.request_timeout_ms == 0u) {
        client->config.request_timeout_ms = 3000u;
    }
    if (client->config.max_attempts == 0u) {
        client->config.max_attempts = 1u;
    }
    return CCBG_STM32_OK;
}

CcbgStm32Status ccbg_stm32_head_object(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key) {
    char uri[CCBG_STM32_MAX_URI_LEN];
    CcbgStm32Status status;
    if (client == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    status = build_uri(uri, sizeof(uri), bucket, key);
    if (status != CCBG_STM32_OK) {
        return status;
    }
    return request_with_retry(client, CCBG_STM32_HTTP_HEAD, uri, 0u, 0, 0, 0, 0);
}

CcbgStm32Status ccbg_stm32_get_object(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key,
    CcbgStm32WriteBodyFn write_body,
    void *body_ctx) {
    char uri[CCBG_STM32_MAX_URI_LEN];
    CcbgStm32Status status;
    if (client == 0 || write_body == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    status = build_uri(uri, sizeof(uri), bucket, key);
    if (status != CCBG_STM32_OK) {
        return status;
    }
    return request_with_retry(client, CCBG_STM32_HTTP_GET, uri, 0u, 0, 0, write_body, body_ctx);
}

CcbgStm32Status ccbg_stm32_put_object_stream(
    CcbgStm32Client *client,
    const char *bucket,
    const char *key,
    uint32_t content_length,
    CcbgStm32ReadBodyFn read_body,
    void *body_ctx) {
    char uri[CCBG_STM32_MAX_URI_LEN];
    CcbgStm32Status status;
    if (client == 0 || read_body == 0) {
        return CCBG_STM32_ERR_INVALID_ARG;
    }
    if (content_length > CCBG_STM32_MAX_OBJECT_BYTES) {
        return CCBG_STM32_ERR_BUFFER_TOO_SMALL;
    }
    status = build_uri(uri, sizeof(uri), bucket, key);
    if (status != CCBG_STM32_OK) {
        return status;
    }
    return request_with_retry(client, CCBG_STM32_HTTP_PUT, uri, content_length, read_body, body_ctx, 0, 0);
}
