[CmdletBinding(DefaultParameterSetName = "Path")]
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
param(
    [Parameter(Mandatory = $true)]
    [string]$Subject,

    [Parameter(ParameterSetName = "Path", Mandatory = $true)]
    [string]$EnvelopePath,

    [Parameter(ParameterSetName = "Inline", Mandatory = $true)]
    [string]$EnvelopeJson,

    [string]$SshIdentityPath = "$HOME\.ssh\ccbg-49-deploy",
    [string]$SshTarget = "root@192.168.1.51",
    [string]$HubDir = "/srv/hermes-redmine-hub"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($PSCmdlet.ParameterSetName -eq "Path") {
    $rawJson = Get-Content -LiteralPath $EnvelopePath -Raw
} else {
    $rawJson = $EnvelopeJson
}

$envelope = $rawJson | ConvertFrom-Json

foreach ($field in @("type", "product_id", "agent_id", "body")) {
    if (-not $envelope.PSObject.Properties.Name.Contains($field)) {
        throw "Envelope JSON is missing required field: $field"
    }
}

if ([string]::IsNullOrWhiteSpace([string]$envelope.body)) {
    throw "Envelope body must not be empty."
}

$payloadBytes = [Text.Encoding]::UTF8.GetByteCount($rawJson)
if ($payloadBytes -gt (240 * 1024)) {
    throw "Envelope exceeds 240 KiB: $payloadBytes bytes"
}

$inputObject = [ordered]@{
    subject = $Subject
    envelopeBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($rawJson))
}

$inputJson = $inputObject | ConvertTo-Json -Compress -Depth 4
$inputBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($inputJson))

$nodeSource = @'
import { connect } from "nats";

const encoder = new TextEncoder();
const input = JSON.parse(Buffer.from(process.argv[2], "base64").toString("utf8"));
const user = process.env.NATS_AGENT_BUS_CODEX_CCBG_USERNAME;
const pass = process.env.NATS_AGENT_BUS_CODEX_CCBG_PASSWORD;

if (!user || !pass) {
  throw new Error("NATS_AGENT_BUS_CODEX_CCBG_USERNAME/PASSWORD are missing in /etc/nats/agent-bus.env");
}

const envelopeText = Buffer.from(input.envelopeBase64, "base64").toString("utf8");
const envelope = JSON.parse(envelopeText);

const nc = await connect({
  servers: "nats://192.168.1.51:4222",
  user,
  pass,
  name: "ccbg-agent-bus-publish"
});

try {
  const js = nc.jetstream();
  const ack = await js.publish(input.subject, encoder.encode(JSON.stringify(envelope)));
  console.log(JSON.stringify({
    subject: input.subject,
    stream: ack.stream,
    sequence: ack.seq,
    payload_bytes: Buffer.byteLength(JSON.stringify(envelope), "utf8"),
    type: envelope.type ?? null,
    product_id: envelope.product_id ?? null,
    agent_id: envelope.agent_id ?? null,
    request_id: envelope.metadata?.request_id ?? envelope.request_id ?? null
  }, null, 2));
} finally {
  await nc.close();
}
'@

$nodeBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($nodeSource))
$remoteTempName = "tmp-ccbg-publish-" + [Guid]::NewGuid().ToString("N") + ".mjs"

$remoteCommand = @"
cd $HubDir
tmp_script="$HubDir/$remoteTempName"
printf '%s' '$nodeBase64' | base64 -d > "`$tmp_script"
trap 'rm -f "`$tmp_script"' EXIT
set -a
. /etc/nats/agent-bus.env
set +a
node "`$tmp_script" '$inputBase64'
"@

ssh -i "$SshIdentityPath" $SshTarget $remoteCommand
