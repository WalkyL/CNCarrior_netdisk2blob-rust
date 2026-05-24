#!/usr/bin/env python3
"""Reference CCBG notify webhook receiver with optional HMAC verification."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Final


EVENT_ID_HEADER: Final[str] = "x-ccbg-notify-event-id"
SIGNATURE_VERSION_HEADER: Final[str] = "x-ccbg-notify-signature-version"
SIGNATURE_HEADER: Final[str] = "x-ccbg-notify-signature"
TIMESTAMP_HEADER: Final[str] = "x-ccbg-notify-timestamp"
SUPPORTED_SIGNATURE_VERSION: Final[str] = "v1"


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def compute_signature(secret: str, timestamp_ms: int, body: bytes) -> str:
    payload_hash = sha256_hex(body)
    string_to_sign = f"{timestamp_ms}.{payload_hash}".encode("utf-8")
    return hmac.new(secret.encode("utf-8"), string_to_sign, hashlib.sha256).hexdigest()


class NotifyWebhookHandler(BaseHTTPRequestHandler):
    signing_secret: str | None = None
    max_age_seconds: int = 300

    def do_POST(self) -> None:  # noqa: N802
        try:
            body = self._read_body()
            payload = self._parse_json(body)
            headers = self.headers
            event_id = headers.get(EVENT_ID_HEADER)
            timestamp_value = headers.get(TIMESTAMP_HEADER)

            if not event_id:
                self._reject(400, f"missing {EVENT_ID_HEADER}")
                return
            if not timestamp_value:
                self._reject(400, f"missing {TIMESTAMP_HEADER}")
                return

            timestamp_ms = self._parse_timestamp(timestamp_value)
            if timestamp_ms is None:
                self._reject(400, f"invalid {TIMESTAMP_HEADER}")
                return

            if not self._timestamp_is_fresh(timestamp_ms):
                self._reject(400, "stale webhook timestamp")
                return

            if self.signing_secret:
                if not self._verify_signature(headers, timestamp_ms, body):
                    return

            output = {
                "received_at_unix_ms": int(time.time() * 1000),
                "event_id": event_id,
                "timestamp_unix_ms": timestamp_ms,
                "alert_count": len(payload.get("alerts", [])),
                "payload": payload,
            }
            print(json.dumps(output, ensure_ascii=True), flush=True)
            self._accept(204)
        except Exception as exc:  # pragma: no cover - defensive logging path
            self._reject(500, f"internal error: {exc}")

    def log_message(self, fmt: str, *args: object) -> None:
        sys.stderr.write(
            "%s - - [%s] %s\n"
            % (self.address_string(), self.log_date_time_string(), fmt % args)
        )

    def _read_body(self) -> bytes:
        content_length = self.headers.get("Content-Length")
        if content_length is None:
            raise ValueError("missing Content-Length")
        length = int(content_length)
        return self.rfile.read(length)

    def _parse_json(self, body: bytes) -> dict:
        try:
            payload = json.loads(body)
        except json.JSONDecodeError as exc:
            raise ValueError(f"invalid JSON body: {exc}") from exc
        if not isinstance(payload, dict):
            raise ValueError("payload must be a JSON object")
        return payload

    def _parse_timestamp(self, raw_value: str) -> int | None:
        try:
            value = int(raw_value)
        except ValueError:
            return None
        return value if value >= 0 else None

    def _timestamp_is_fresh(self, timestamp_ms: int) -> bool:
        now_ms = int(time.time() * 1000)
        delta_ms = abs(now_ms - timestamp_ms)
        return delta_ms <= self.max_age_seconds * 1000

    def _verify_signature(self, headers, timestamp_ms: int, body: bytes) -> bool:
        version = headers.get(SIGNATURE_VERSION_HEADER)
        signature = headers.get(SIGNATURE_HEADER)
        if version != SUPPORTED_SIGNATURE_VERSION:
            self._reject(400, f"unsupported {SIGNATURE_VERSION_HEADER}: {version!r}")
            return False
        if not signature:
            self._reject(400, f"missing {SIGNATURE_HEADER}")
            return False

        expected_signature = compute_signature(self.signing_secret or "", timestamp_ms, body)
        if not hmac.compare_digest(signature, expected_signature):
            self._reject(401, "signature mismatch")
            return False
        return True

    def _accept(self, status_code: int) -> None:
        self.send_response(status_code)
        self.end_headers()

    def _reject(self, status_code: int, message: str) -> None:
        response = json.dumps({"error": message}, ensure_ascii=True).encode("utf-8")
        self.send_response(status_code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Reference receiver for carrier-cloud-blob-gateway notify webhook."
    )
    parser.add_argument("--host", default="127.0.0.1", help="Listen host, default 127.0.0.1")
    parser.add_argument("--port", type=int, default=61110, help="Listen port, default 61110")
    parser.add_argument(
        "--secret",
        default=os.getenv("CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET", ""),
        help="Optional signing secret; if empty, signature verification is skipped",
    )
    parser.add_argument(
        "--max-age-seconds",
        type=int,
        default=300,
        help="Reject timestamps older/newer than this window, default 300 seconds",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    NotifyWebhookHandler.signing_secret = args.secret or None
    NotifyWebhookHandler.max_age_seconds = max(args.max_age_seconds, 1)
    server = ThreadingHTTPServer((args.host, args.port), NotifyWebhookHandler)
    print(
        json.dumps(
            {
                "listening": f"http://{args.host}:{args.port}",
                "signature_verification": bool(NotifyWebhookHandler.signing_secret),
                "timestamp_max_age_seconds": NotifyWebhookHandler.max_age_seconds,
                "dedupe_note": "persist event_id before forwarding to downstream systems",
            },
            ensure_ascii=True,
        ),
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
