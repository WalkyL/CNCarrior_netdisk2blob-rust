/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#include "ccbg_stm32_client.h"

#include "esp_http_client.h"
#include "esp_log.h"
#include "mbedtls/md.h"
#include "mbedtls/sha256.h"
#include "sdkconfig.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

#define CCBG_ESP32S3_IO_CHUNK_BYTES 1024u
#define CCBG_ESP32S3_MAX_OBJECT_BYTES (32u * 1024u)

static const char *TAG = "ccbg_esp32s3";

typedef struct CcbgEsp32HttpCtx {
    const char *host;
    char url[320];
} CcbgEsp32HttpCtx;

typedef struct DemoBody {
    const uint8_t *data;
    size_t len;
    size_t offset;
} DemoBody;

static int esp_sha256_hex(void *ctx, const uint8_t *data, size_t len, char out_hex65[65]) {
    static const char hex[] = "0123456789abcdef";
    uint8_t digest[32];
    size_t i;
    (void)ctx;
    if (mbedtls_sha256(data, len, digest, 0) != 0) {
        return -1;
    }
    for (i = 0; i < sizeof(digest); i++) {
        out_hex65[i * 2u] = hex[(digest[i] >> 4u) & 0x0fu];
        out_hex65[i * 2u + 1u] = hex[digest[i] & 0x0fu];
    }
    out_hex65[64] = '\0';
    return 0;
}

static int esp_hmac_sha256(
    void *ctx,
    const uint8_t *key,
    size_t key_len,
    const uint8_t *data,
    size_t data_len,
    uint8_t out32[32]) {
    const mbedtls_md_info_t *md_info;
    (void)ctx;
    md_info = mbedtls_md_info_from_type(MBEDTLS_MD_SHA256);
    if (md_info == NULL) {
        return -1;
    }
    return mbedtls_md_hmac(md_info, key, key_len, data, data_len, out32);
}

static int esp_utc_now(void *ctx, char out_amz_date17[17], char out_date9[9]) {
    time_t now;
    struct tm tm_now;
    (void)ctx;
    now = time(NULL);
    if (now <= 0 || gmtime_r(&now, &tm_now) == NULL) {
        return -1;
    }
    if (strftime(out_amz_date17, 17u, "%Y%m%dT%H%M%SZ", &tm_now) != 16u) {
        return -1;
    }
    if (strftime(out_date9, 9u, "%Y%m%d", &tm_now) != 8u) {
        return -1;
    }
    return 0;
}

static esp_http_client_method_t esp_method(CcbgStm32Method method) {
    switch (method) {
    case CCBG_STM32_HTTP_HEAD:
        return HTTP_METHOD_HEAD;
    case CCBG_STM32_HTTP_GET:
        return HTTP_METHOD_GET;
    case CCBG_STM32_HTTP_PUT:
        return HTTP_METHOD_PUT;
    default:
        return HTTP_METHOD_GET;
    }
}

static int esp_http_request(
    void *ctx,
    const CcbgStm32HttpRequest *request,
    CcbgStm32HttpResponse *response) {
    CcbgEsp32HttpCtx *http_ctx = (CcbgEsp32HttpCtx *)ctx;
    esp_http_client_config_t config;
    esp_http_client_handle_t client;
    esp_err_t err;
    uint8_t buffer[CCBG_ESP32S3_IO_CHUNK_BYTES];
    int read_result;
    int status;
    uint32_t sent = 0u;
    size_t i;
    int written;

    if (http_ctx == NULL || request == NULL || response == NULL) {
        return -1;
    }
    written = snprintf(http_ctx->url, sizeof(http_ctx->url), "http://%s%s", http_ctx->host, request->uri);
    if (written < 0 || (size_t)written >= sizeof(http_ctx->url)) {
        return -1;
    }

    memset(&config, 0, sizeof(config));
    config.url = http_ctx->url;
    config.method = esp_method(request->method);
    config.timeout_ms = (int)request->timeout_ms;
    client = esp_http_client_init(&config);
    if (client == NULL) {
        return -1;
    }
    for (i = 0; i < request->header_count; i++) {
        esp_http_client_set_header(client, request->headers[i].name, request->headers[i].value);
    }
    err = esp_http_client_open(client, (int)request->content_length);
    if (err != ESP_OK) {
        esp_http_client_cleanup(client);
        return -1;
    }
    while (request->read_body != NULL && sent < request->content_length) {
        size_t got = request->read_body(request->body_ctx, buffer, sizeof(buffer));
        if (got == 0u) {
            break;
        }
        if (esp_http_client_write(client, (const char *)buffer, (int)got) != (int)got) {
            esp_http_client_close(client);
            esp_http_client_cleanup(client);
            return -1;
        }
        sent += (uint32_t)got;
    }
    if (esp_http_client_fetch_headers(client) < 0) {
        esp_http_client_close(client);
        esp_http_client_cleanup(client);
        return -1;
    }
    while (response->write_body != NULL) {
        read_result = esp_http_client_read(client, (char *)buffer, sizeof(buffer));
        if (read_result < 0) {
            esp_http_client_close(client);
            esp_http_client_cleanup(client);
            return -1;
        }
        if (read_result == 0) {
            break;
        }
        if (response->write_body(response->body_ctx, buffer, (size_t)read_result) != (size_t)read_result) {
            esp_http_client_close(client);
            esp_http_client_cleanup(client);
            return -1;
        }
    }
    status = esp_http_client_get_status_code(client);
    esp_http_client_close(client);
    esp_http_client_cleanup(client);
    response->status_code = status;
    return 0;
}

static size_t demo_read_body(void *ctx, uint8_t *buffer, size_t capacity) {
    DemoBody *body = (DemoBody *)ctx;
    size_t remaining = body->len - body->offset;
    size_t take = remaining < capacity ? remaining : capacity;
    if (take > 0u) {
        memcpy(buffer, body->data + body->offset, take);
        body->offset += take;
    }
    return take;
}

static size_t demo_write_body(void *ctx, const uint8_t *data, size_t len) {
    size_t *total = (size_t *)ctx;
    (void)data;
    *total += len;
    return len;
}

void app_main(void) {
#if CONFIG_CCBG_DEMO_AUTO_RUN
    static const uint8_t payload[] = "hello from esp32-s3 client-only";
    CcbgEsp32HttpCtx http_ctx = { CONFIG_CCBG_GATEWAY_HOST, {0} };
    CcbgStm32Client client;
    DemoBody upload_body = { payload, sizeof(payload) - 1u, 0u };
    size_t downloaded = 0u;
    CcbgStm32Config client_config = {
        CONFIG_CCBG_GATEWAY_HOST,
        CONFIG_CCBG_S3_REGION,
        CONFIG_CCBG_S3_ACCESS_KEY,
        CONFIG_CCBG_S3_SECRET_KEY,
        3000u,
        2u,
        100u
    };
    CcbgStm32Platform platform = {
        esp_sha256_hex,
        esp_hmac_sha256,
        esp_utc_now,
        esp_http_request,
        NULL,
        NULL,
        &http_ctx
    };
    if (sizeof(payload) - 1u > CCBG_ESP32S3_MAX_OBJECT_BYTES) {
        ESP_LOGE(TAG, "demo payload exceeds max object budget");
        return;
    }
    if (ccbg_stm32_client_init(&client, &client_config, &platform) != CCBG_STM32_OK) {
        ESP_LOGE(TAG, "client init failed");
        return;
    }
    if (ccbg_stm32_put_object_stream(
            &client,
            CONFIG_CCBG_DEMO_BUCKET,
            CONFIG_CCBG_DEMO_KEY,
            (uint32_t)upload_body.len,
            demo_read_body,
            &upload_body) != CCBG_STM32_OK) {
        ESP_LOGE(TAG, "put object failed");
        return;
    }
    if (ccbg_stm32_head_object(&client, CONFIG_CCBG_DEMO_BUCKET, CONFIG_CCBG_DEMO_KEY) != CCBG_STM32_OK) {
        ESP_LOGE(TAG, "head object failed");
        return;
    }
    if (ccbg_stm32_get_object(
            &client,
            CONFIG_CCBG_DEMO_BUCKET,
            CONFIG_CCBG_DEMO_KEY,
            demo_write_body,
            &downloaded) != CCBG_STM32_OK) {
        ESP_LOGE(TAG, "get object failed");
        return;
    }
    ESP_LOGI(TAG, "demo completed, downloaded=%u", (unsigned)downloaded);
#else
    ESP_LOGI(TAG, "CCBG demo disabled by CONFIG_CCBG_DEMO_AUTO_RUN");
#endif
}
