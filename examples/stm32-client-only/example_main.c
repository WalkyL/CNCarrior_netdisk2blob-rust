/*
 * SPDX-License-Identifier: LicenseRef-CCBG-Commercial
 * Copyright (c) 2026 walky
 */

#include "ccbg_stm32_client.h"

#include <stdio.h>
#include <string.h>

typedef struct ExampleBody {
    const uint8_t *data;
    size_t len;
    size_t offset;
} ExampleBody;

static int fake_sha256_hex(void *ctx, const uint8_t *data, size_t len, char out_hex65[65]) {
    size_t i;
    (void)ctx;
    (void)data;
    (void)len;
    for (i = 0; i < 64u; i++) {
        out_hex65[i] = '0';
    }
    out_hex65[64] = '\0';
    return 0;
}

static int fake_hmac_sha256(
    void *ctx,
    const uint8_t *key,
    size_t key_len,
    const uint8_t *data,
    size_t data_len,
    uint8_t out32[32]) {
    (void)ctx;
    (void)key;
    (void)key_len;
    (void)data;
    (void)data_len;
    memset(out32, 0x42, 32u);
    return 0;
}

static int fixed_utc_now(void *ctx, char out_amz_date17[17], char out_date9[9]) {
    (void)ctx;
    memcpy(out_amz_date17, "20260527T000000Z", 17u);
    memcpy(out_date9, "20260527", 9u);
    return 0;
}

static size_t read_body(void *ctx, uint8_t *buffer, size_t capacity) {
    ExampleBody *body = (ExampleBody *)ctx;
    size_t remaining = body->len - body->offset;
    size_t take = remaining < capacity ? remaining : capacity;
    if (take > 0u) {
        memcpy(buffer, body->data + body->offset, take);
        body->offset += take;
    }
    return take;
}

static size_t write_body(void *ctx, const uint8_t *data, size_t len) {
    (void)ctx;
    printf("received %u bytes\n", (unsigned)len);
    (void)data;
    return len;
}

static int fake_http(void *ctx, const CcbgStm32HttpRequest *request, CcbgStm32HttpResponse *response) {
    uint8_t chunk[CCBG_STM32_IO_CHUNK_BYTES];
    uint32_t sent = 0u;
    (void)ctx;
    printf("HTTP method=%d uri=%s headers=%u content_length=%u timeout_ms=%u\n",
           (int)request->method,
           request->uri,
           (unsigned)request->header_count,
           (unsigned)request->content_length,
           (unsigned)request->timeout_ms);
    while (request->read_body != 0 && sent < request->content_length) {
        size_t got = request->read_body(request->body_ctx, chunk, sizeof(chunk));
        if (got == 0u) {
            break;
        }
        sent += (uint32_t)got;
    }
    if (response->write_body != 0) {
        static const uint8_t sample[] = "ok";
        response->write_body(response->body_ctx, sample, sizeof(sample) - 1u);
    }
    response->status_code = 200;
    return 0;
}

int main(void) {
    static const uint8_t payload[] = "hello from stm32 client";
    ExampleBody body = { payload, sizeof(payload) - 1u, 0u };
    CcbgStm32Client client;
    CcbgStm32Config config = {
        "192.168.1.43:61080",
        "us-east-1",
        "ccbg",
        "change-me",
        3000u,
        2u,
        100u
    };
    CcbgStm32Platform platform = {
        fake_sha256_hex,
        fake_hmac_sha256,
        fixed_utc_now,
        fake_http,
        0,
        0,
        0
    };
    if (ccbg_stm32_client_init(&client, &config, &platform) != CCBG_STM32_OK) {
        return 1;
    }
    if (ccbg_stm32_head_object(&client, "root", "stm32/demo.txt") != CCBG_STM32_OK) {
        return 2;
    }
    if (ccbg_stm32_get_object(&client, "root", "stm32/demo.txt", write_body, 0) != CCBG_STM32_OK) {
        return 3;
    }
    if (ccbg_stm32_put_object_stream(
            &client,
            "root",
            "stm32/demo.txt",
            (uint32_t)body.len,
            read_body,
            &body) != CCBG_STM32_OK) {
        return 4;
    }
    return 0;
}
