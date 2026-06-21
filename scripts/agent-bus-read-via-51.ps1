[CmdletBinding(DefaultParameterSetName = "Subject")]
param(
    [Parameter(ParameterSetName = "Sequence", Mandatory = $true)]
    [int[]]$Sequence,

    [Parameter(ParameterSetName = "Subject", Mandatory = $true)]
    [string]$Subject,

    [Parameter(ParameterSetName = "Subject")]
    [string]$RequestId,

    [Parameter(ParameterSetName = "Subject")]
    [ValidateRange(1, 100)]
    [int]$Limit = 10,

    [string]$Stream = "AGENT_BUS",
    [string]$SshIdentityPath = "$HOME\.ssh\ccbg-49-deploy",
    [string]$SshTarget = "root@192.168.1.51",
    [string]$HubDir = "/srv/hermes-redmine-hub"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$inputObject = [ordered]@{
    stream = $Stream
}

if ($PSCmdlet.ParameterSetName -eq "Sequence") {
    $inputObject.sequence = @($Sequence)
} else {
    $inputObject.subject = $Subject
    $inputObject.limit = $Limit
    if ($RequestId) {
        $inputObject.requestId = $RequestId
    }
}

$inputJson = $inputObject | ConvertTo-Json -Compress -Depth 8
$inputBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($inputJson))

$nodeSource = @'
import { connect } from "nats";

const decoder = new TextDecoder();

function parsePayload(stored) {
  const text = decoder.decode(stored.data);
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function payloadRequestId(payload) {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    return null;
  }
  if (payload.metadata && typeof payload.metadata === "object" && payload.metadata.request_id) {
    return payload.metadata.request_id;
  }
  return payload.request_id ?? null;
}

function normalizeHeaders(stored) {
  const result = {};
  if (!stored.header) {
    return result;
  }
  for (const [key, value] of stored.header) {
    result[key] = value;
  }
  return result;
}

const input = JSON.parse(Buffer.from(process.argv[2], "base64").toString("utf8"));
const user = process.env.NATS_AGENT_BUS_CODEX_CCBG_USERNAME;
const pass = process.env.NATS_AGENT_BUS_CODEX_CCBG_PASSWORD;

if (!user || !pass) {
  throw new Error("NATS_AGENT_BUS_CODEX_CCBG_USERNAME/PASSWORD are missing in /etc/nats/agent-bus.env");
}

const nc = await connect({
  servers: "nats://192.168.1.51:4222",
  user,
  pass,
  name: "ccbg-agent-bus-read"
});

try {
  const jsm = await nc.jetstreamManager();

  if (Array.isArray(input.sequence) && input.sequence.length > 0) {
    const messages = [];
    for (const seq of input.sequence) {
      const stored = await jsm.streams.getMessage(input.stream, { seq });
      messages.push({
        seq: stored.seq,
        subject: stored.subject,
        timestamp: stored.time,
        headers: normalizeHeaders(stored),
        payload: parsePayload(stored)
      });
    }
    console.log(JSON.stringify({
      stream: input.stream,
      mode: "sequence",
      messages
    }, null, 2));
    process.exit(0);
  }

  const info = await jsm.streams.info(input.stream);
  const firstSeq = info.state.first_seq ?? 1;
  const lastSeq = info.state.last_seq ?? 0;
  const messages = [];

  for (let seq = lastSeq; seq >= firstSeq; seq -= 1) {
    if (messages.length >= input.limit) {
      break;
    }

    let stored;
    try {
      stored = await jsm.streams.getMessage(input.stream, { seq });
    } catch {
      continue;
    }

    if (stored.subject !== input.subject) {
      continue;
    }

    const payload = parsePayload(stored);
    const requestId = payloadRequestId(payload);
    if (input.requestId && requestId !== input.requestId) {
      continue;
    }

    messages.push({
      seq: stored.seq,
      subject: stored.subject,
      timestamp: stored.time,
      headers: normalizeHeaders(stored),
      payload
    });
  }

  console.log(JSON.stringify({
    stream: input.stream,
    mode: "subject",
    subject: input.subject,
    request_id: input.requestId ?? null,
    first_seq: firstSeq,
    last_seq: lastSeq,
    total_matches: messages.length,
    messages
  }, null, 2));
} finally {
  await nc.close();
}
'@

$nodeBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($nodeSource))
$remoteTempName = "tmp-ccbg-read-" + [Guid]::NewGuid().ToString("N") + ".mjs"

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
