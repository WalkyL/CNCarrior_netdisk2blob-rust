# `.51` Agent Bus Read / Reply Ops

Date: `2026-06-22`

## Purpose

Provide a repeatable `ccbg`-side way to inspect `.51` Agent Bus history and
send bounded replies without installing a local `nats` CLI on this Windows
host.

This flow is for the current live CCBG NATS identity:

- `agent_id = codex-ccbg`
- inbox subject `agents.codex-ccbg.inbox`
- room subject `rooms.product-ccbg.events`

## Current Constraint

This workstation does not keep `.51` NATS credentials in local environment
variables.

Instead, the live credentials already exist on `.51` in:

- `/etc/nats/agent-bus.env`

The scripts below SSH to `.51`, source that env file there, and run the NATS
operation inside `/srv/hermes-redmine-hub`, where the `nats` Node dependency
is already present.

The scripts do not print the password and do not leave a remote temp file.

## Prerequisites

- local SSH key works:
  - `ssh -i "$HOME\.ssh\ccbg-49-deploy" root@192.168.1.51`
- `.51` has:
  - `/etc/nats/agent-bus.env`
  - `/srv/hermes-redmine-hub`
- `/etc/nats/agent-bus.env` includes:
  - `NATS_AGENT_BUS_CODEX_CCBG_USERNAME`
  - `NATS_AGENT_BUS_CODEX_CCBG_PASSWORD`

## Read History

Read specific JetStream sequences:

```powershell
.\scripts\agent-bus-read-via-51.ps1 -Sequence 41,64,66
```

Read the newest messages on the live CCBG inbox:

```powershell
.\scripts\agent-bus-read-via-51.ps1 `
  -Subject agents.codex-ccbg.inbox `
  -Limit 5
```

Read a specific request on the live CCBG inbox:

```powershell
.\scripts\agent-bus-read-via-51.ps1 `
  -Subject agents.codex-ccbg.inbox `
  -RequestId ccbg-doc-live-inbox-20260621-161436 `
  -Limit 5
```

## Why History Scan Instead Of Subscribe

The current `.51` `codex-ccbg` NATS user can:

- subscribe to:
  - `agents.codex-ccbg.inbox`
  - `broadcast.*`
  - `rooms.*.events`
- publish to:
  - `agents.*.inbox`
  - `broadcast.*`
  - `rooms.*.events`
- inspect JetStream history through:
  - `$JS.API.INFO`
  - `$JS.API.STREAM.INFO.AGENT_BUS`
  - `$JS.API.STREAM.MSG.GET.AGENT_BUS`

That means:

- live intake of the CCBG inbox is allowed directly
- history inspection of any stored Agent Bus subject is possible through
  JetStream message lookup

For operator troubleshooting, history scan is often enough and does not require
keeping a long-running local subscriber process on this Windows host.

## Publish A Bounded Reply

Prepare an `AgentEnvelope` JSON file first. Example reply to
`product-manager-agent-50`:

```json
{
  "type": "steering_note",
  "product_id": "product-manager-agent",
  "agent_id": "product-manager-agent-50",
  "redmine_issue_id": null,
  "todo_id": null,
  "run_id": null,
  "telegram_chat_id": null,
  "telegram_topic_id": null,
  "body": "CCBG side verified the live .51 inbox and confirmed the requested messages are present.",
  "refs": {},
  "metadata": {
    "request_id": "ccbg-nats-reply-example-20260622",
    "reply_kind": "ccbg_inbox_confirmation",
    "source_product_id": "ccbg",
    "source_agent_id": "codex-ccbg"
  }
}
```

Publish it:

```powershell
.\scripts\agent-bus-publish-via-51.ps1 `
  -Subject agents.product-manager-agent-50.inbox `
  -EnvelopePath D:\tmp\ccbg-reply.json
```

The script checks:

- JSON parses successfully
- `type`, `product_id`, `agent_id`, and `body` exist
- payload stays within the `240 KiB` envelope limit

## Verified On 2026-06-22

The following checks already passed:

1. `agents.codex-ccbg.inbox` history read succeeded on `.51`
2. the live resend request was found:
   - `request_id = ccbg-doc-live-inbox-20260621-161436`
   - `seq = 64`
3. the multipart investigation note was found:
   - `request_id = ccbg-s3-invalidpart-check-20260621-201653`
   - `seq = 66`
4. the earlier direct credential handoff was found:
   - `seq = 41`
5. a bounded reply back to `product-manager-agent-50` was published and
   verified:
   - subject `agents.product-manager-agent-50.inbox`
   - `request_id = ccbg-nats-reply-20260622-013100`
   - `seq = 69`

## Scope Boundary

This ops path is intentionally narrow:

- it does not store `.51` NATS secrets in this repo
- it does not require a local `nats` binary
- it does not replace a real long-running daemon consumer

It is meant for:

- operator verification
- inbox triage
- bounded reply / acknowledgment publishing
