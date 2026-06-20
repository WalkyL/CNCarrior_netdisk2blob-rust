// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

pub mod client;
pub mod error;
pub mod http;
pub mod prompts;
pub mod resources;
pub mod schema;
pub mod server;

pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
pub const MCP_SERVER_NAME: &str = "carrier-cloud-blob-gateway-mcp";
