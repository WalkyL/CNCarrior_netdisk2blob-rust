# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
idf_component_register(
    SRCS
        "ccbg_esp32s3_demo.c"
        "../../stm32-client-only/ccbg_stm32_client.c"
    INCLUDE_DIRS
        "."
        "../../stm32-client-only"
    REQUIRES
        esp_http_client
        mbedtls
)
