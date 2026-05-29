#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
import argparse
import datetime as dt
import hashlib
import http.client
import hmac
import json
import os
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as et
from pathlib import Path


def now_utc_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()


def pick_port() -> int:
    for _ in range(200):
        port = random.randint(62000, 65450)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            try:
                sock.bind(("127.0.0.1", port))
            except OSError:
                continue
            return port
    raise RuntimeError("failed to allocate free port in 60000..65534")


def run_cmd(cmd, env=None, cwd=None, timeout=60):
    return subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        timeout=timeout,
        text=True,
        capture_output=True,
        check=False,
    )


class SigV4Client:
    def __init__(self, endpoint: str, access_key: str, secret_key: str, region: str = "us-east-1"):
        self.endpoint = endpoint.rstrip("/")
        self.parsed = urllib.parse.urlparse(self.endpoint)
        self.host = self.parsed.netloc
        self.access_key = access_key
        self.secret_key = secret_key
        self.region = region
        self.service = "s3"

    def _sig_key(self, datestamp: str):
        k_date = hmac.new(("AWS4" + self.secret_key).encode(), datestamp.encode(), hashlib.sha256).digest()
        k_region = hmac.new(k_date, self.region.encode(), hashlib.sha256).digest()
        k_service = hmac.new(k_region, self.service.encode(), hashlib.sha256).digest()
        return hmac.new(k_service, b"aws4_request", hashlib.sha256).digest()

    def _canonical_qs(self, query_pairs):
        if not query_pairs:
            return ""
        encoded = []
        for key, value in query_pairs:
            k = urllib.parse.quote(str(key), safe="-_.~")
            if value is None:
                v = ""
            else:
                v = urllib.parse.quote(str(value), safe="-_.~")
            encoded.append((k, v))
        encoded.sort(key=lambda item: (item[0], item[1]))
        return "&".join(f"{k}={v}" for k, v in encoded)

    def request(
        self,
        method: str,
        path: str,
        query_pairs=None,
        body: bytes = b"",
        extra_headers=None,
        host_header=None,
    ):
        method = method.upper()
        query_pairs = query_pairs or []
        extra_headers = extra_headers or {}
        payload_hash = hashlib.sha256(body).hexdigest()
        t = dt.datetime.now(dt.timezone.utc)
        amz_date = t.strftime("%Y%m%dT%H%M%SZ")
        datestamp = t.strftime("%Y%m%d")
        canonical_uri = urllib.parse.quote(path, safe="/-_.~")
        canonical_qs = self._canonical_qs(query_pairs)
        signed_host = host_header or self.host
        headers = {
            "host": signed_host,
            "x-amz-content-sha256": payload_hash,
            "x-amz-date": amz_date,
        }
        for k, v in extra_headers.items():
            headers[k.lower()] = str(v).strip()
        signed_header_keys = sorted(headers.keys())
        canonical_headers = "".join(f"{k}:{headers[k]}\n" for k in signed_header_keys)
        signed_headers = ";".join(signed_header_keys)
        canonical_request = "\n".join(
            [method, canonical_uri, canonical_qs, canonical_headers, signed_headers, payload_hash]
        )
        credential_scope = f"{datestamp}/{self.region}/{self.service}/aws4_request"
        string_to_sign = "\n".join(
            [
                "AWS4-HMAC-SHA256",
                amz_date,
                credential_scope,
                hashlib.sha256(canonical_request.encode()).hexdigest(),
            ]
        )
        signature = hmac.new(self._sig_key(datestamp), string_to_sign.encode(), hashlib.sha256).hexdigest()
        auth = (
            "AWS4-HMAC-SHA256 "
            f"Credential={self.access_key}/{credential_scope}, "
            f"SignedHeaders={signed_headers}, Signature={signature}"
        )
        req_headers = {k: v for k, v in headers.items()}
        req_headers["Authorization"] = auth
        query = self._canonical_qs(query_pairs)
        url = f"{self.endpoint}{path}" + (f"?{query}" if query else "")
        conn = http.client.HTTPConnection(self.parsed.hostname, self.parsed.port, timeout=20)
        try:
            conn.request(method, path + (f"?{query}" if query else ""), body=body, headers=req_headers)
            resp = conn.getresponse()
            return resp.status, dict(resp.getheaders()), resp.read()
        finally:
            conn.close()


def extract_xml_code(body: bytes):
    return find_xml_text(body, "Code")


def find_xml_text(body: bytes, local_name: str):
    try:
        root = et.fromstring(body)
    except et.ParseError:
        return None
    for element in root.iter():
        name = element.tag.rsplit("}", 1)[-1]
        if name == local_name and element.text:
            return element.text.strip()
    return None


def make_step(name, ok, detail="", skipped=False):
    return {"name": name, "ok": bool(ok), "skipped": bool(skipped), "detail": detail}


def normalize_external_step(raw):
    if not isinstance(raw, dict):
        return None
    name = raw.get("name")
    if not isinstance(name, str) or not name.strip():
        return None
    return make_step(
        name.strip(),
        bool(raw.get("ok", False)),
        str(raw.get("detail", "")),
        bool(raw.get("skipped", False)),
    )


def header_value(headers, name):
    name = name.lower()
    for key, value in headers.items():
        if key.lower() == name:
            return value
    return ""


def wait_health(url, timeout_sec=60):
    deadline = time.time() + timeout_sec
    last_err = ""
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status == 200:
                    return
                last_err = f"http {resp.status}"
        except Exception as err:  # noqa: BLE001
            last_err = str(err)
        time.sleep(0.5)
    raise RuntimeError(f"gateway health check timeout: {last_err}")


def internal_smoke(client: SigV4Client, strict_protocol: bool = False):
    steps = []
    bucket = "root"
    base = "smoke/internal"
    plain_key = f"{base}/plain.txt"
    copy_key = f"{base}/copy.txt"
    mp_key = f"{base}/multipart.txt"
    data = b"hello-smoke-internal"
    put_status, _, _ = client.request("PUT", f"/{bucket}/{plain_key}", body=data)
    steps.append(make_step("put_object", put_status == 200, f"status={put_status}"))
    get_status, _, get_body = client.request("GET", f"/{bucket}/{plain_key}")
    steps.append(make_step("get_object", get_status == 200 and get_body == data, f"status={get_status} bytes={len(get_body)}"))
    head_status, _, _ = client.request("HEAD", f"/{bucket}/{plain_key}")
    steps.append(make_step("head_object", head_status == 200, f"status={head_status}"))
    listb_status, _, _ = client.request("GET", "/")
    steps.append(make_step("list_buckets", listb_status == 200, f"status={listb_status}"))
    listo_status, _, _ = client.request("GET", f"/{bucket}", query_pairs=[("list-type", "2"), ("prefix", base)])
    steps.append(make_step("list_objects_v2", listo_status == 200, f"status={listo_status}"))
    copy_headers = {"x-amz-copy-source": f"/{bucket}/{plain_key}"}
    copy_status, _, _ = client.request("PUT", f"/{bucket}/{copy_key}", extra_headers=copy_headers)
    steps.append(make_step("copy_object", copy_status == 200, f"status={copy_status}"))
    range_status, _, range_body = client.request("GET", f"/{bucket}/{plain_key}", extra_headers={"range": "bytes=0-4"})
    if range_status == 206 and range_body == b"hello":
        steps.append(make_step("range_get_valid", True, f"status={range_status} body={range_body!r}"))
    elif (not strict_protocol) and range_status == 200:
        steps.append(make_step("range_get_valid", False, f"status={range_status} body={range_body!r}", skipped=True))
    else:
        steps.append(make_step("range_get_valid", False, f"status={range_status} body={range_body!r}"))
    miss_status, _, miss_body = client.request("GET", f"/{bucket}/{base}/missing.txt")
    miss_code = extract_xml_code(miss_body)
    steps.append(make_step("error_no_such_key", miss_status == 404 and miss_code == "NoSuchKey", f"status={miss_status} code={miss_code}"))
    bad_range_status, _, bad_range_body = client.request("GET", f"/{bucket}/{plain_key}", extra_headers={"range": "bytes=999999-1000000"})
    bad_range_code = extract_xml_code(bad_range_body)
    if bad_range_status == 416 and bad_range_code == "InvalidRange":
        steps.append(make_step("error_invalid_range", True, f"status={bad_range_status} code={bad_range_code}"))
    elif (not strict_protocol) and bad_range_status == 200:
        steps.append(make_step("error_invalid_range", False, f"status={bad_range_status} code={bad_range_code}", skipped=True))
    else:
        steps.append(make_step("error_invalid_range", False, f"status={bad_range_status} code={bad_range_code}"))
    malformed_status, _, malformed_body = client.request("GET", f"/{bucket}/{plain_key}", extra_headers={"range": "abc"})
    malformed_code = extract_xml_code(malformed_body)
    if malformed_status == 400 and malformed_code == "InvalidRequest":
        steps.append(make_step("error_malformed_range", True, f"status={malformed_status} code={malformed_code}"))
    elif (not strict_protocol) and malformed_status == 200:
        steps.append(make_step("error_malformed_range", False, f"status={malformed_status} code={malformed_code}", skipped=True))
    else:
        steps.append(make_step("error_malformed_range", False, f"status={malformed_status} code={malformed_code}"))
    init_status, _, init_body = client.request("POST", f"/{bucket}/{mp_key}", query_pairs=[("uploads", None)])
    upload_id = None
    if init_status == 200:
        upload_id = find_xml_text(init_body, "UploadId")
    if init_status == 200 and upload_id:
        steps.append(make_step("multipart_initiate", True, f"status={init_status} upload_id={upload_id}"))
    elif (not strict_protocol) and init_status in (400, 405):
        steps.append(make_step("multipart_initiate", False, f"status={init_status} upload_id={upload_id}", skipped=True))
    else:
        steps.append(make_step("multipart_initiate", False, f"status={init_status} upload_id={upload_id}"))
    etag1 = None
    etag2 = None
    if upload_id:
        p1_status, p1_headers, _ = client.request(
            "PUT",
            f"/{bucket}/{mp_key}",
            query_pairs=[("partNumber", "1"), ("uploadId", upload_id)],
            body=b"part-1-",
        )
        etag1 = header_value(p1_headers, "etag").strip("\"")
        steps.append(make_step("multipart_upload_part_1", p1_status == 200 and bool(etag1), f"status={p1_status} etag={etag1}"))
        p2_status, p2_headers, _ = client.request(
            "PUT",
            f"/{bucket}/{mp_key}",
            query_pairs=[("partNumber", "2"), ("uploadId", upload_id)],
            body=b"part-2",
        )
        etag2 = header_value(p2_headers, "etag").strip("\"")
        steps.append(make_step("multipart_upload_part_2", p2_status == 200 and bool(etag2), f"status={p2_status} etag={etag2}"))
    else:
        steps.append(make_step("multipart_upload_part_1", False, "missing upload id", skipped=True))
        steps.append(make_step("multipart_upload_part_2", False, "missing upload id", skipped=True))
    if upload_id and etag1 and etag2:
        complete_xml = (
            "<CompleteMultipartUpload>"
            f"<Part><PartNumber>1</PartNumber><ETag>\"{etag1}\"</ETag></Part>"
            f"<Part><PartNumber>2</PartNumber><ETag>\"{etag2}\"</ETag></Part>"
            "</CompleteMultipartUpload>"
        ).encode()
        c_status, _, _ = client.request(
            "POST",
            f"/{bucket}/{mp_key}",
            query_pairs=[("uploadId", upload_id)],
            body=complete_xml,
            extra_headers={"content-type": "application/xml"},
        )
        steps.append(make_step("multipart_complete", c_status == 200, f"status={c_status}"))
    else:
        steps.append(make_step("multipart_complete", False, "multipart parts unavailable", skipped=True))
    a_init_status, _, a_init_body = client.request("POST", f"/{bucket}/{base}/abort.txt", query_pairs=[("uploads", None)])
    a_upload_id = None
    if a_init_status == 200:
        a_upload_id = find_xml_text(a_init_body, "UploadId")
    if a_upload_id:
        a_status, _, _ = client.request(
            "DELETE",
            f"/{bucket}/{base}/abort.txt",
            query_pairs=[("uploadId", a_upload_id)],
        )
        steps.append(make_step("multipart_abort", a_status == 204, f"status={a_status}"))
    else:
        steps.append(make_step("multipart_abort", False, "abort upload init failed", skipped=True))
    style_status, _, _ = client.request("GET", f"/{bucket}", query_pairs=[("list-type", "2"), ("max-keys", "1")])
    steps.append(make_step("addressing_style_path", style_status == 200, f"status={style_status}"))
    virtual_host = f"{bucket}.localhost:{client.parsed.port}"
    vh_status, _, vh_body = client.request(
        "GET",
        f"/{plain_key}",
        host_header=virtual_host,
    )
    steps.append(
        make_step(
            "addressing_style_virtual_hosted",
            vh_status == 200 and vh_body == data,
            f"status={vh_status} host={virtual_host} bytes={len(vh_body)}",
        )
    )
    for key in (plain_key, copy_key, mp_key):
        d_status, _, _ = client.request("DELETE", f"/{bucket}/{key}")
        steps.append(make_step(f"delete_{key.split('/')[-1]}", d_status in (204, 404), f"status={d_status}"))
    return steps


def aws_cli_smoke(endpoint: str, env):
    aws = shutil.which("aws")
    if not aws:
        return {"client": "aws-cli", "status": "skipped", "steps": [make_step("client_presence", False, "aws not found", skipped=True)]}
    steps = []
    base = "smoke/awscli"
    tmpdir = tempfile.mkdtemp(prefix="ccbg-awscli-")
    src = Path(tmpdir) / "data.txt"
    src.write_bytes(b"hello-from-aws-cli")
    common = [aws, "s3api", "--endpoint-url", endpoint, "--region", "us-east-1", "--no-cli-pager"]
    try:
        put = run_cmd(common + ["put-object", "--bucket", "root", "--key", f"{base}/a.txt", "--body", str(src)], env=env)
        steps.append(make_step("put_object", put.returncode == 0, put.stderr.strip() or put.stdout.strip()))
        get = run_cmd(common + ["get-object", "--bucket", "root", "--key", f"{base}/a.txt", str(Path(tmpdir) / "out.txt")], env=env)
        ok_get = get.returncode == 0 and (Path(tmpdir) / "out.txt").read_bytes() == b"hello-from-aws-cli"
        steps.append(make_step("get_object", ok_get, get.stderr.strip() or get.stdout.strip()))
        cp = run_cmd(common + ["copy-object", "--bucket", "root", "--key", f"{base}/b.txt", "--copy-source", f"root/{base}/a.txt"], env=env)
        steps.append(make_step("copy_object", cp.returncode == 0, cp.stderr.strip() or cp.stdout.strip()))
        rg = run_cmd(common + ["get-object", "--bucket", "root", "--key", f"{base}/a.txt", "--range", "bytes=0-4", str(Path(tmpdir) / "range.txt")], env=env)
        ok_rg = rg.returncode == 0 and (Path(tmpdir) / "range.txt").read_bytes() == b"hello"
        steps.append(make_step("range_get", ok_rg, rg.stderr.strip() or rg.stdout.strip()))
        bad = run_cmd(common + ["get-object", "--bucket", "root", "--key", f"{base}/a.txt", "--range", "bytes=999999-1000000", str(Path(tmpdir) / "bad.txt")], env=env)
        steps.append(make_step("error_invalid_range", bad.returncode != 0 and "InvalidRange" in (bad.stderr + bad.stdout), (bad.stderr + bad.stdout).strip()))
        mp_init = run_cmd(common + ["create-multipart-upload", "--bucket", "root", "--key", f"{base}/mp.txt"], env=env)
        mp_upload_id = None
        if mp_init.returncode == 0:
            try:
                mp_upload_id = json.loads(mp_init.stdout).get("UploadId")
            except Exception:  # noqa: BLE001
                mp_upload_id = None
        steps.append(make_step("multipart_create", mp_init.returncode == 0 and bool(mp_upload_id), (mp_init.stderr.strip() or mp_init.stdout.strip())[:1000]))
        p1 = Path(tmpdir) / "mp-part1.bin"
        p2 = Path(tmpdir) / "mp-part2.bin"
        p1.write_bytes(b"aws-cli-part-1-")
        p2.write_bytes(b"aws-cli-part-2")
        etag1 = None
        etag2 = None
        if mp_upload_id:
            mp_p1 = run_cmd(
                common + ["upload-part", "--bucket", "root", "--key", f"{base}/mp.txt", "--part-number", "1", "--upload-id", mp_upload_id, "--body", str(p1)],
                env=env,
            )
            if mp_p1.returncode == 0:
                try:
                    etag1 = json.loads(mp_p1.stdout).get("ETag")
                except Exception:  # noqa: BLE001
                    etag1 = None
            mp_p2 = run_cmd(
                common + ["upload-part", "--bucket", "root", "--key", f"{base}/mp.txt", "--part-number", "2", "--upload-id", mp_upload_id, "--body", str(p2)],
                env=env,
            )
            if mp_p2.returncode == 0:
                try:
                    etag2 = json.loads(mp_p2.stdout).get("ETag")
                except Exception:  # noqa: BLE001
                    etag2 = None
            steps.append(make_step("multipart_upload_part", mp_p1.returncode == 0 and mp_p2.returncode == 0 and bool(etag1) and bool(etag2), ((mp_p1.stderr + mp_p1.stdout + "\n" + mp_p2.stderr + mp_p2.stdout).strip())[:1000]))
            if etag1 and etag2:
                complete_file = Path(tmpdir) / "complete.json"
                complete_file.write_text(
                    json.dumps({"Parts": [{"ETag": etag1, "PartNumber": 1}, {"ETag": etag2, "PartNumber": 2}]}),
                    encoding="utf-8",
                )
                mp_complete = run_cmd(
                    common + ["complete-multipart-upload", "--bucket", "root", "--key", f"{base}/mp.txt", "--upload-id", mp_upload_id, "--multipart-upload", f"file://{complete_file}"],
                    env=env,
                )
                steps.append(make_step("multipart_complete", mp_complete.returncode == 0, (mp_complete.stderr.strip() or mp_complete.stdout.strip())[:1000]))
            else:
                steps.append(make_step("multipart_complete", False, "missing etag(s)", skipped=True))
        else:
            steps.append(make_step("multipart_upload_part", False, "missing upload id", skipped=True))
            steps.append(make_step("multipart_complete", False, "missing upload id", skipped=True))
        abort_init = run_cmd(common + ["create-multipart-upload", "--bucket", "root", "--key", f"{base}/abort.txt"], env=env)
        abort_upload_id = None
        if abort_init.returncode == 0:
            try:
                abort_upload_id = json.loads(abort_init.stdout).get("UploadId")
            except Exception:  # noqa: BLE001
                abort_upload_id = None
        if abort_upload_id:
            abort = run_cmd(common + ["abort-multipart-upload", "--bucket", "root", "--key", f"{base}/abort.txt", "--upload-id", abort_upload_id], env=env)
            steps.append(make_step("multipart_abort", abort.returncode == 0, (abort.stderr.strip() or abort.stdout.strip())[:1000]))
        else:
            steps.append(make_step("multipart_abort", False, (abort_init.stderr.strip() or abort_init.stdout.strip())[:1000], skipped=True))
        d_a = run_cmd(common + ["delete-object", "--bucket", "root", "--key", f"{base}/a.txt"], env=env)
        d_b = run_cmd(common + ["delete-object", "--bucket", "root", "--key", f"{base}/b.txt"], env=env)
        d_mp = run_cmd(common + ["delete-object", "--bucket", "root", "--key", f"{base}/mp.txt"], env=env)
        head = run_cmd(common + ["head-object", "--bucket", "root", "--key", f"{base}/a.txt"], env=env)
        delete_ok = d_a.returncode == 0 and d_b.returncode == 0 and d_mp.returncode == 0 and head.returncode != 0
        steps.append(make_step("delete_object", delete_ok, ((d_a.stderr + d_b.stderr + d_mp.stderr + head.stderr + head.stdout).strip())[:1000]))
        for key in ("a.txt", "b.txt"):
            run_cmd(common + ["delete-object", "--bucket", "root", "--key", f"{base}/{key}"], env=env)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
    status = "passed" if all(step["ok"] or step["skipped"] for step in steps) else "failed"
    return {"client": "aws-cli", "status": status, "steps": steps}


def boto3_smoke(endpoint: str, access: str, secret: str):
    try:
        import boto3
        from botocore.config import Config
        from botocore.exceptions import ClientError
    except Exception:
        return {"client": "boto3", "status": "skipped", "steps": [make_step("client_presence", False, "boto3/botocore not found", skipped=True)]}
    steps = []
    cfg = Config(signature_version="s3v4", s3={"addressing_style": "path"})
    s3 = boto3.client(
        "s3",
        endpoint_url=endpoint,
        aws_access_key_id=access,
        aws_secret_access_key=secret,
        region_name="us-east-1",
        config=cfg,
    )
    base = "smoke/boto3"
    try:
        s3.put_object(Bucket="root", Key=f"{base}/a.txt", Body=b"hello-from-boto3")
        steps.append(make_step("put_object", True, "ok"))
        out = s3.get_object(Bucket="root", Key=f"{base}/a.txt")["Body"].read()
        steps.append(make_step("get_object", out == b"hello-from-boto3", f"bytes={len(out)}"))
        s3.copy_object(Bucket="root", Key=f"{base}/b.txt", CopySource={"Bucket": "root", "Key": f"{base}/a.txt"})
        steps.append(make_step("copy_object", True, "ok"))
        r = s3.get_object(Bucket="root", Key=f"{base}/a.txt", Range="bytes=0-4")
        steps.append(make_step("range_get", r["Body"].read() == b"hello", f"status={r['ResponseMetadata']['HTTPStatusCode']}"))
        try:
            s3.get_object(Bucket="root", Key=f"{base}/missing.txt")
            steps.append(make_step("error_no_such_key", False, "expected NoSuchKey"))
        except ClientError as err:
            code = err.response.get("Error", {}).get("Code")
            http = err.response.get("ResponseMetadata", {}).get("HTTPStatusCode")
            steps.append(make_step("error_no_such_key", code == "NoSuchKey" and http == 404, f"http={http} code={code}"))
        mp_init = s3.create_multipart_upload(Bucket="root", Key=f"{base}/mp.txt")
        mp_upload_id = mp_init.get("UploadId")
        steps.append(make_step("multipart_create", bool(mp_upload_id), f"upload_id={mp_upload_id}"))
        mp_part1 = None
        mp_part2 = None
        if mp_upload_id:
            mp_part1 = s3.upload_part(Bucket="root", Key=f"{base}/mp.txt", UploadId=mp_upload_id, PartNumber=1, Body=b"boto3-part-1-")
            mp_part2 = s3.upload_part(Bucket="root", Key=f"{base}/mp.txt", UploadId=mp_upload_id, PartNumber=2, Body=b"boto3-part-2")
            etag1 = mp_part1.get("ETag")
            etag2 = mp_part2.get("ETag")
            steps.append(make_step("multipart_upload_part", bool(etag1) and bool(etag2), f"etag1={etag1} etag2={etag2}"))
            if etag1 and etag2:
                s3.complete_multipart_upload(
                    Bucket="root",
                    Key=f"{base}/mp.txt",
                    UploadId=mp_upload_id,
                    MultipartUpload={"Parts": [{"PartNumber": 1, "ETag": etag1}, {"PartNumber": 2, "ETag": etag2}]},
                )
                steps.append(make_step("multipart_complete", True, "ok"))
            else:
                steps.append(make_step("multipart_complete", False, "missing etag(s)", skipped=True))
        else:
            steps.append(make_step("multipart_upload_part", False, "missing upload id", skipped=True))
            steps.append(make_step("multipart_complete", False, "missing upload id", skipped=True))
        abort_init = s3.create_multipart_upload(Bucket="root", Key=f"{base}/abort.txt")
        abort_upload_id = abort_init.get("UploadId")
        if abort_upload_id:
            s3.abort_multipart_upload(Bucket="root", Key=f"{base}/abort.txt", UploadId=abort_upload_id)
            steps.append(make_step("multipart_abort", True, "ok"))
        else:
            steps.append(make_step("multipart_abort", False, "missing upload id", skipped=True))
        s3.delete_object(Bucket="root", Key=f"{base}/a.txt")
        s3.delete_object(Bucket="root", Key=f"{base}/b.txt")
        s3.delete_object(Bucket="root", Key=f"{base}/mp.txt")
        try:
            s3.get_object(Bucket="root", Key=f"{base}/a.txt")
            steps.append(make_step("delete_object", False, "object still readable after delete"))
        except ClientError as err:
            code = err.response.get("Error", {}).get("Code")
            http = err.response.get("ResponseMetadata", {}).get("HTTPStatusCode")
            steps.append(make_step("delete_object", code == "NoSuchKey" and http == 404, f"http={http} code={code}"))
        steps.append(make_step("addressing_style_path", True, "Config(s3.addressing_style=path)"))
    finally:
        for key in ("a.txt", "b.txt", "mp.txt"):
            try:
                s3.delete_object(Bucket="root", Key=f"{base}/{key}")
            except Exception:
                pass
    status = "passed" if all(step["ok"] or step["skipped"] for step in steps) else "failed"
    return {"client": "boto3", "status": status, "steps": steps}


def rclone_smoke(endpoint: str, access: str, secret: str):
    rclone = shutil.which("rclone")
    if not rclone:
        return {"client": "rclone", "status": "skipped", "steps": [make_step("client_presence", False, "rclone not found", skipped=True)]}
    steps = []
    tmpdir = tempfile.mkdtemp(prefix="ccbg-rclone-")
    cfg = Path(tmpdir) / "rclone.conf"
    cfg.write_text(
        "\n".join(
            [
                "[ccbg]",
                "type = s3",
                "provider = AWS",
                "env_auth = false",
                f"access_key_id = {access}",
                f"secret_access_key = {secret}",
                "region = us-east-1",
                f"endpoint = {endpoint}",
                "acl = private",
                "no_check_bucket = true",
                "force_path_style = true",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    try:
        src = Path(tmpdir) / "a.txt"
        src.write_bytes(b"hello-from-rclone")
        put = run_cmd([rclone, "--config", str(cfg), "copyto", str(src), "ccbg:root/smoke/rclone/a.txt"], timeout=60)
        steps.append(make_step("put_object", put.returncode == 0, put.stderr.strip() or put.stdout.strip()))
        cat = run_cmd([rclone, "--config", str(cfg), "cat", "ccbg:root/smoke/rclone/a.txt"], timeout=60)
        steps.append(make_step("get_object", cat.returncode == 0 and cat.stdout.encode() == b"hello-from-rclone", cat.stderr.strip() or f"bytes={len(cat.stdout.encode())}"))
        r_range = run_cmd([rclone, "--config", str(cfg), "cat", "--offset", "0", "--count", "5", "ccbg:root/smoke/rclone/a.txt"], timeout=60)
        range_detail = (r_range.stderr.strip() or r_range.stdout.strip())[:1000]
        if r_range.returncode == 0:
            steps.append(make_step("range_get", r_range.stdout.encode() == b"hello", range_detail))
        else:
            unsupported = "unknown flag" in (r_range.stderr + r_range.stdout).lower() or "flag provided but not defined" in (r_range.stderr + r_range.stdout).lower()
            steps.append(make_step("range_get", False, range_detail, skipped=unsupported))
        cp = run_cmd([rclone, "--config", str(cfg), "copyto", "ccbg:root/smoke/rclone/a.txt", "ccbg:root/smoke/rclone/b.txt"], timeout=60)
        steps.append(make_step("copy_object", cp.returncode == 0, cp.stderr.strip() or cp.stdout.strip()))
        mp_src = Path(tmpdir) / "mp.bin"
        mp_src.write_bytes(b"x" * (6 * 1024 * 1024))
        mp_put = run_cmd(
            [
                rclone,
                "--config",
                str(cfg),
                "copyto",
                "--s3-upload-cutoff",
                "5M",
                "--s3-chunk-size",
                "5M",
                str(mp_src),
                "ccbg:root/smoke/rclone/mp.bin",
            ],
            timeout=60,
        )
        steps.append(
            make_step(
                "multipart_via_rclone",
                mp_put.returncode == 0,
                (
                    "rclone multipart is provider-driven via upload cutoff/chunk-size threshold; "
                    + (mp_put.stderr.strip() or mp_put.stdout.strip())
                )[:1000],
            )
        )
        ls = run_cmd([rclone, "--config", str(cfg), "ls", "ccbg:root/smoke/rclone"], timeout=60)
        steps.append(make_step("list_objects", ls.returncode == 0 and "a.txt" in ls.stdout and "b.txt" in ls.stdout and "mp.bin" in ls.stdout, ls.stderr.strip() or ls.stdout.strip()))
        del_a = run_cmd([rclone, "--config", str(cfg), "deletefile", "ccbg:root/smoke/rclone/a.txt"], timeout=60)
        del_b = run_cmd([rclone, "--config", str(cfg), "deletefile", "ccbg:root/smoke/rclone/b.txt"], timeout=60)
        del_mp = run_cmd([rclone, "--config", str(cfg), "deletefile", "ccbg:root/smoke/rclone/mp.bin"], timeout=60)
        lsf = run_cmd([rclone, "--config", str(cfg), "lsf", "ccbg:root/smoke/rclone"], timeout=60)
        delete_ok = del_a.returncode == 0 and del_b.returncode == 0 and del_mp.returncode == 0 and "a.txt" not in lsf.stdout and "b.txt" not in lsf.stdout and "mp.bin" not in lsf.stdout
        steps.append(make_step("delete_object", delete_ok, (del_a.stderr + del_b.stderr + del_mp.stderr + lsf.stderr + lsf.stdout).strip()[:1000]))
        for key in ("a.txt", "b.txt", "mp.bin"):
            run_cmd([rclone, "--config", str(cfg), "deletefile", f"ccbg:root/smoke/rclone/{key}"], timeout=60)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
    status = "passed" if all(step["ok"] or step["skipped"] for step in steps) else "failed"
    return {"client": "rclone", "status": status, "steps": steps}


RUST_SDK_REQUIRED_STEPS = {
    "put_object",
    "get_object",
    "delete_object",
    "multipart_create",
    "multipart_upload_part",
    "multipart_complete",
    "multipart_abort",
    "range_get",
}


def rust_sdk_smoke(endpoint: str, access: str, secret: str):
    command = os.environ.get("CCBG_SMOKE_RUST_SDK_COMMAND", "").strip()
    if not command:
        return {
            "client": "aws-sdk-rust",
            "status": "skipped",
            "steps": [
                make_step(
                    "client_presence",
                    False,
                    "set CCBG_SMOKE_RUST_SDK_COMMAND to run an aws-sdk-s3 based smoke binary",
                    skipped=True,
                )
            ],
        }
    env = {
        **os.environ,
        "CCBG_SMOKE_ENDPOINT": endpoint,
        "CCBG_SMOKE_ACCESS_KEY_ID": access,
        "CCBG_SMOKE_SECRET_ACCESS_KEY": secret,
        "CCBG_SMOKE_REGION": "us-east-1",
    }
    result = run_cmd(["bash", "-lc", command], env=env, timeout=300)
    expected = RUST_SDK_REQUIRED_STEPS
    detail = (result.stderr.strip() or result.stdout.strip())[:4000]
    ok = result.returncode == 0
    parsed_steps = []
    parsed = None
    if ok:
        try:
            parsed = json.loads(result.stdout)
        except Exception:  # noqa: BLE001
            parsed = None
        raw_steps = []
        if isinstance(parsed, dict):
            raw_steps = parsed.get("steps", [])
        elif isinstance(parsed, list):
            raw_steps = parsed
        for raw in raw_steps if isinstance(raw_steps, list) else []:
            normalized = normalize_external_step(raw)
            if normalized is not None:
                parsed_steps.append(normalized)
        if not parsed_steps:
            ok = False
            parsed_steps.append(
                make_step(
                    "contract_json_steps",
                    False,
                    "command exited 0 but no valid JSON steps found on stdout",
                )
            )
        else:
            names = {step["name"] for step in parsed_steps}
            missing = sorted(expected - names)
            parsed_steps.append(make_step("contract_expected_steps", len(missing) == 0, f"missing={','.join(missing)}" if missing else "all required steps present"))
            if missing:
                ok = False
    return {
        "client": "aws-sdk-rust",
        "status": "passed" if ok else "failed",
        "steps": parsed_steps if parsed_steps else [make_step("external_rust_sdk_smoke", ok, detail)],
    }


def parse_required(raw: str):
    if not raw:
        return set()
    return {item.strip() for item in raw.split(",") if item.strip()}


def main():
    parser = argparse.ArgumentParser(description="S3 smoke matrix for gatewayd (stub provider only)")
    parser.add_argument("--report-path", default="target/s3-smoke/report.json")
    parser.add_argument("--require-clients", default=os.environ.get("CCBG_SMOKE_REQUIRE_CLIENTS", ""))
    parser.add_argument("--skip-build", action="store_true", default=os.environ.get("CCBG_SMOKE_SKIP_BUILD") == "1")
    parser.add_argument("--keep-temp", action="store_true")
    parser.add_argument(
        "--allow-protocol-skips",
        action="store_true",
        default=os.environ.get("CCBG_SMOKE_ALLOW_PROTOCOL_SKIPS") == "1",
        help="mark incomplete range/multipart protocol checks as skipped instead of failed",
    )
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    report_path = root / args.report_path
    report_path.parent.mkdir(parents=True, exist_ok=True)
    required_clients = parse_required(args.require_clients)
    started_at = now_utc_iso()
    run_root = Path(tempfile.mkdtemp(prefix="ccbg-s3-smoke-", dir=str((root / "target" / "s3-smoke"))))
    gateway_proc = None
    failures = []
    clients = []
    endpoint = ""

    try:
        if not args.skip_build:
            build = run_cmd(["cargo", "build", "-p", "gatewayd"], cwd=root, timeout=1800)
            if build.returncode != 0:
                raise RuntimeError(f"cargo build failed:\n{build.stdout}\n{build.stderr}")
        bin_path = root / "target" / "debug" / "gatewayd"
        if not bin_path.exists():
            raise RuntimeError(f"gatewayd binary not found: {bin_path}")

        s3_port = pick_port()
        admin_port = pick_port()
        metrics_port = pick_port()
        endpoint = f"http://127.0.0.1:{s3_port}"
        env = os.environ.copy()
        env.update(
            {
                "CCBG_PRIMARY_PROVIDER": "stub",
                "CCBG_BIND_ADDR": f"127.0.0.1:{s3_port}",
                "CCBG_ADMIN_BIND_ADDR": f"127.0.0.1:{admin_port}",
                "CCBG_METRICS_BIND_ADDR": f"127.0.0.1:{metrics_port}",
                "CCBG_METADATA_DB_PATH": str(run_root / "metadata" / "ccbg.db"),
                "CCBG_CONTROL_PLANE_FILE": str(run_root / "control-plane.json"),
                "CCBG_CREDENTIALS_DIR": str(run_root / "provider-credentials"),
                "CCBG_BODY_SPOOL_DIR": str(run_root / "body-spool"),
                "CCBG_MAX_SPOOLED_OBJECT_BYTES": str(16 * 1024 * 1024),
                "CCBG_S3_ACCESS_KEY_ID": "smoke-access",
                "CCBG_S3_SECRET_ACCESS_KEY": "smoke-secret",
                "RUST_LOG": os.environ.get("RUST_LOG", "warn"),
            }
        )
        (run_root / "metadata").mkdir(parents=True, exist_ok=True)
        (run_root / "provider-credentials").mkdir(parents=True, exist_ok=True)
        (run_root / "body-spool").mkdir(parents=True, exist_ok=True)

        gateway_log = (run_root / "gatewayd.log").open("w", encoding="utf-8")
        gateway_proc = subprocess.Popen([str(bin_path)], cwd=root, env=env, stdout=gateway_log, stderr=subprocess.STDOUT)
        wait_health(f"{endpoint}/healthz", timeout_sec=90)
        wait_health(f"http://127.0.0.1:{metrics_port}/readyz", timeout_sec=90)

        internal_client = SigV4Client(endpoint=endpoint, access_key="smoke-access", secret_key="smoke-secret")
        internal_steps = internal_smoke(internal_client, strict_protocol=not args.allow_protocol_skips)
        clients.append({"client": "internal-sigv4", "status": "passed" if all(s["ok"] or s["skipped"] for s in internal_steps) else "failed", "steps": internal_steps})
        clients.append(
            aws_cli_smoke(
                endpoint,
                {
                    **env,
                    "AWS_ACCESS_KEY_ID": "smoke-access",
                    "AWS_SECRET_ACCESS_KEY": "smoke-secret",
                    "AWS_EC2_METADATA_DISABLED": "true",
                    "AWS_DEFAULT_REGION": "us-east-1",
                },
            )
        )
        clients.append(boto3_smoke(endpoint, "smoke-access", "smoke-secret"))
        clients.append(rclone_smoke(endpoint, "smoke-access", "smoke-secret"))
        clients.append(rust_sdk_smoke(endpoint, "smoke-access", "smoke-secret"))

        for name in required_clients:
            matched = next((c for c in clients if c["client"] == name), None)
            if matched is None or matched["status"] == "skipped":
                failures.append(f"required client missing/skipped: {name}")
    finally:
        if gateway_proc is not None:
            gateway_proc.terminate()
            try:
                gateway_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                gateway_proc.kill()
                gateway_proc.wait(timeout=5)
        if not args.keep_temp:
            shutil.rmtree(run_root, ignore_errors=True)

    for client in clients:
        if client["status"] == "failed":
            failures.append(f"client failed: {client['client']}")

    ended_at = now_utc_iso()
    report = {
        "started_at": started_at,
        "ended_at": ended_at,
        "endpoint": endpoint,
        "required_clients": sorted(required_clients),
        "overall_status": "passed" if not failures else "failed",
        "failures": failures,
        "clients": clients,
    }
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    total = len(clients)
    passed = sum(1 for c in clients if c["status"] == "passed")
    skipped = sum(1 for c in clients if c["status"] == "skipped")
    failed = sum(1 for c in clients if c["status"] == "failed")
    print(f"S3 smoke summary: overall={report['overall_status']} clients={total} passed={passed} skipped={skipped} failed={failed}")
    print(f"Report: {report_path}")
    if failures:
        for item in failures:
            print(f"- {item}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
