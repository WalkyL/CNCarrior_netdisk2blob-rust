#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
import argparse
import datetime as dt
import hashlib
import hmac
import http.client
import os
import random
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.parse
from pathlib import Path


def pick_port() -> int:
    for _ in range(200):
        port = random.randint(62000, 65450)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            try:
                sock.bind(("127.0.0.1", port))
            except OSError:
                continue
            return port
    raise RuntimeError("failed to allocate free port in 62000..65450")


def run_cmd(cmd, cwd=None, timeout=1800):
    return subprocess.run(
        cmd,
        cwd=cwd,
        timeout=timeout,
        text=True,
        capture_output=True,
        check=False,
    )


def run_streaming_cmd(cmd, cwd=None, timeout=1800):
    return subprocess.run(
        cmd,
        cwd=cwd,
        timeout=timeout,
        text=True,
        check=False,
    )


def wait_health(url: str, timeout_sec: int = 90):
    parsed = urllib.parse.urlparse(url)
    deadline = time.time() + timeout_sec
    last_err = ""
    while time.time() < deadline:
        conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=2)
        try:
            conn.request("GET", parsed.path)
            resp = conn.getresponse()
            body = resp.read()
            if resp.status == 200:
                return body
            last_err = f"http {resp.status}"
        except Exception as err:  # noqa: BLE001
            last_err = str(err)
        finally:
            conn.close()
        time.sleep(0.5)
    raise RuntimeError(f"health check timeout: {last_err}")


def sigv4_list_buckets(endpoint: str, access_key: str, secret_key: str, region: str = "us-east-1"):
    parsed = urllib.parse.urlparse(endpoint)
    host = parsed.netloc
    t = dt.datetime.now(dt.timezone.utc)
    amz_date = t.strftime("%Y%m%dT%H%M%SZ")
    datestamp = t.strftime("%Y%m%d")
    payload_hash = hashlib.sha256(b"").hexdigest()
    canonical_request = "\n".join(
        [
            "GET",
            "/",
            "",
            f"host:{host}\n"
            f"x-amz-content-sha256:{payload_hash}\n"
            f"x-amz-date:{amz_date}\n",
            "host;x-amz-content-sha256;x-amz-date",
            payload_hash,
        ]
    )
    scope = f"{datestamp}/{region}/s3/aws4_request"
    string_to_sign = "\n".join(
        [
            "AWS4-HMAC-SHA256",
            amz_date,
            scope,
            hashlib.sha256(canonical_request.encode()).hexdigest(),
        ]
    )
    k_date = hmac.new(("AWS4" + secret_key).encode(), datestamp.encode(), hashlib.sha256).digest()
    k_region = hmac.new(k_date, region.encode(), hashlib.sha256).digest()
    k_service = hmac.new(k_region, b"s3", hashlib.sha256).digest()
    k_signing = hmac.new(k_service, b"aws4_request", hashlib.sha256).digest()
    signature = hmac.new(k_signing, string_to_sign.encode(), hashlib.sha256).hexdigest()
    auth = (
        "AWS4-HMAC-SHA256 "
        f"Credential={access_key}/{scope}, "
        "SignedHeaders=host;x-amz-content-sha256;x-amz-date, "
        f"Signature={signature}"
    )
    headers = {
        "host": host,
        "x-amz-content-sha256": payload_hash,
        "x-amz-date": amz_date,
        "Authorization": auth,
    }
    conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=10)
    try:
        conn.request("GET", "/", headers=headers)
        resp = conn.getresponse()
        body = resp.read().decode("utf-8", errors="replace")
        if resp.status != 200:
            raise RuntimeError(f"list-buckets failed status={resp.status} body={body[:300]}")
    finally:
        conn.close()


def main():
    parser = argparse.ArgumentParser(description="Container image smoke for gatewayd")
    parser.add_argument("--runtime", required=True, choices=["docker", "podman"])
    parser.add_argument("--file", required=True, help="Dockerfile/Containerfile path relative to repo root")
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    runtime = args.runtime
    image_file = root / args.file
    if not image_file.exists():
        raise RuntimeError(f"container file not found: {image_file}")
    if shutil.which(runtime) is None:
        raise RuntimeError(f"container runtime not found: {runtime}")

    host_port = pick_port()
    target_dir = root / "target"
    target_dir.mkdir(parents=True, exist_ok=True)
    work_dir = Path(tempfile.mkdtemp(prefix=f"ccbg-{runtime}-smoke-", dir=str(target_dir)))
    (work_dir / "data").mkdir(parents=True, exist_ok=True)
    (work_dir / "body-spool").mkdir(parents=True, exist_ok=True)
    os.chmod(work_dir / "data", 0o777)
    os.chmod(work_dir / "body-spool", 0o777)
    container_name = f"ccbg-smoke-{runtime}-{random.randint(1000, 9999)}"
    access_key = "smoke-access"
    secret_key = "smoke-secret"
    endpoint = f"http://127.0.0.1:{host_port}"

    try:
        print(f"building image with {runtime}: {args.file} -> {args.tag}", flush=True)
        build = run_streaming_cmd(
            [runtime, "build", "-f", str(image_file), "-t", args.tag, "."],
            cwd=root,
            timeout=3600,
        )
        if build.returncode != 0:
            raise RuntimeError(f"{runtime} build failed with exit code {build.returncode}")

        print(f"starting container {container_name} on {endpoint}", flush=True)
        run = run_cmd(
            [
                runtime,
                "run",
                "-d",
                "--rm",
                "--name",
                container_name,
                "-p",
                f"{host_port}:61080",
                "-v",
                f"{work_dir / 'data'}:/srv/gateway/data",
                "-v",
                f"{work_dir / 'body-spool'}:/srv/gateway/body-spool",
                "-e",
                "CCBG_PRIMARY_PROVIDER=stub",
                "-e",
                "CCBG_BIND_ADDR=0.0.0.0:61080",
                "-e",
                "CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081",
                "-e",
                "CCBG_AUTH_CALLBACK_BIND_ADDR=127.0.0.1:61082",
                "-e",
                "CCBG_METRICS_BIND_ADDR=127.0.0.1:61083",
                "-e",
                "CCBG_ADMIN_MODE=off",
                "-e",
                "CCBG_ONEDRIVE_ENABLED=false",
                "-e",
                "CCBG_ONEDRIVE_REPLICATION_ENABLED=false",
                "-e",
                "CCBG_SYNC_TARGETS=",
                "-e",
                "CCBG_FALLBACK_READ_ORDER=",
                "-e",
                "CCBG_S3_ACCESS_KEY_ID=smoke-access",
                "-e",
                "CCBG_S3_SECRET_ACCESS_KEY=smoke-secret",
                "-e",
                "CCBG_CONTROL_PLANE_FILE=/srv/gateway/data/control-plane.json",
                "-e",
                "CCBG_CREDENTIALS_DIR=/srv/gateway/data/provider-credentials",
                "-e",
                "CCBG_METADATA_DB_PATH=/srv/gateway/data/ccbg.db",
                "-e",
                "CCBG_BODY_SPOOL_DIR=/srv/gateway/body-spool",
                "-e",
                "CCBG_PROVIDER_BRIDGE_CATALOG_DIR=/srv/gateway/config/provider-bridges",
                "-e",
                "CCBG_PROVIDER_CAPABILITY_CATALOG_DIR=/srv/gateway/config/provider-capabilities",
                "-e",
                "CCBG_BROWSER_FLOW_CATALOG_DIR=/srv/gateway/config/browser-flows",
                args.tag,
            ],
            timeout=120,
        )
        if run.returncode != 0:
            raise RuntimeError(f"{runtime} run failed:\n{run.stdout}\n{run.stderr}")

        try:
            wait_health(f"{endpoint}/healthz", timeout_sec=120)
            sigv4_list_buckets(endpoint, access_key, secret_key)
        except Exception:
            logs = run_cmd([runtime, "logs", container_name], timeout=30)
            print(logs.stdout, end="")
            print(logs.stderr, end="")
            raise
        print(f"{runtime} smoke passed: {args.file} -> {args.tag} endpoint={endpoint}")
    finally:
        run_cmd([runtime, "rm", "-f", container_name], timeout=30)
        shutil.rmtree(work_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
