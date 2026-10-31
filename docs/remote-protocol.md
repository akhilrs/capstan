# Remote protocol v1

Status: approved (v1)

This document is the wire contract between a host that runs controllers and a client that follows them from elsewhere
(a phone app, a watcher). It describes the frozen API of the kernel event feed and the paged snapshot (the plan-64
modules `feed_cursor`, `event_feed` and `snapshot_feed`) and the daemon `feed` route over them. A change to that API
needs a follow-up on the feed core and the feed cursor; this document is then revised with it. The examples below are
checked against the code by `rust/crates/daemon/tests/remote_protocol_doc.rs`, which fails when the document and the code
drift.

The stream framing (sections 1 and 2) is new in v1 and is implemented by a later CLI plan; that plan's acceptance checks
its output against this document.

## 1. Transport

The client runs one command on the host over SSH and reads its standard output:

```
ssh <host> cstan events [--cursor <c1.…>] [--root <path>]... [--poll-ms <n>]
```

The command name and flags are the later CLI plan's contract: `--cursor` resumes from a cursor (section 3), each
`--root` names a project root to include besides the ones discovery finds (section 10), `--poll-ms` is the poll
interval. The stream is read only (section 9).

- Standard output is UTF-8 JSON lines: one JSON object per line, ended by `\n`, no blank lines. A line is at most
  65536 bytes including the newline. The server never writes a longer one (an event is at most 49152 bytes, section 10): a line that would pass
  65536 bytes is not written, and the stream ends with an `error` line of code `internal` (exit code 1) instead.
- Standard error is diagnostics for a human only. A client must not parse it and must not fail on its content.
- Exit codes: `0` the client closed the stream or the command was told to stop; `1` a fatal stream `error` line was
  written first; `2` the command line was invalid (no stream is written); `255` is SSH's own failure (no connection, no
  such command). A client treats any end of the stream without a final `cursor` line as an interruption and reconnects
  with the last cursor it saw.
- The stream runs until the client closes it. The server writes `heartbeat` lines while nothing else happens.

## 2. Line kinds

Every line is an object with `"v":1` and a `"kind"`. Field names are camelCase. A line has the fields below and may have
more (section 7).

| kind | fields | meaning |
|---|---|---|
| `hello` | `version`, `host`, `projects` | first line: the tool's version text, the host name, and the number of `project` lines that follow |
| `project` | `projectId`, `root`, `state`, `epoch`, `head` | one per project found; `state` is one of the project states below; `epoch` and `head` are set only when `state` is `ready`, else `null` |
| `snapshot` | `projectId`, `section`, `key`, `item` | one snapshot item of a project; `section` is `waiting`, `agents`, `plans` or `run` |
| `snapshotEnd` | `projectId`, `head` | every item of the project's snapshot was sent; `head` is the snapshot head `H` (section 4) |
| `event` | `projectId`, `event` | one public event, the envelope of section 5 |
| `cursor` | `cursor` | the cursor text `c1.…` (section 3) after the lines before it |
| `heartbeat` | `ts` | nothing changed; `ts` is the host time, UTC, ISO 8601 |
| `resync` | `projectId`, `reason` | the client must drop what it holds of the project and read its snapshot again (section 6); `projectId` is `null` for the whole stream |
| `error` | `code`, `message` | a failure; `code` is in section 6, `message` is for a person and is plain text |

Project states (the wire names of the discovery endpoint states):

| state | meaning |
|---|---|
| `ready` | the controller answers and accepted the operator key |
| `no_key` | no usable operator key was found for the root |
| `silent` | nothing listens, or no answer came in time |
| `refused` | the controller did not accept the key |
| `unsupported` | the controller answers but does not know the `feed` route |
| `gone` | the root or its controller directory is missing |

Discovery probes a controller with `feed head`. It reports a controller that answers `unauthorized` or `forbidden` as
`refused` (the key was not accepted for the operator route) and one that answers `unknown_command` as `unsupported`.
Every other refusal, and no answer, is reported as `silent`. A project whose id is not known (it has no cursor entry
and no `project.json` file) is not listed.

Order of a stream start: `hello`, then every `project` line, then for each `ready` project its `snapshot` lines and
`snapshotEnd` (when there is no cursor entry for it, or a `resync` was sent), then `event` lines, then a `cursor` line
after each batch. Lines of different projects may interleave after the `project` lines; the lines of one project keep
their order.

```json
{"v":1,"kind":"hello","version":"0.1.0","host":"build-host","projects":2}
{"v":1,"kind":"project","projectId":"proj-a","root":"/srv/work/alpha","state":"ready","epoch":"daeca3b2245f3519","head":26}
{"v":1,"kind":"project","projectId":"proj-b","root":"/srv/work/beta","state":"silent","epoch":null,"head":null}
```

## 3. The multi-project cursor

One cursor carries the position of every project, so a client keeps one text.

- Codec `c1.`: the text `c1.` and the unpadded base64url of the compact JSON `{"p":{<projectId>:entry,...}}`. Keys
  are sorted and there is no whitespace, so one cursor has one text.
- An entry has the fields `e`, `s`, `k`, `h`, `r` and `d`, and no others:
  - `e`: the ledger epoch of the project, 16 lowercase hex characters;
  - `s`: the last sequence read (a non-negative integer);
  - `k`: the sub-event of sequence `s` last read, so `(s, k)` is the position the next read starts after; omitted when 0;
  - `h`: the history anchor of `s`, 16 lowercase hex characters; it is `0000000000000000` at `s` 0 and is not checked there;
  - `r`: the project root;
  - `d`: Unix seconds since the project was found unavailable; omitted when the project is available.
- A cursor that does not start with `c1.`, is not base64url, is not JSON, has another top-level key than `p`, an
  entry with another field, a missing `e`, `s`, `h` or `r`, or a value of the wrong type, does not decode. Neither does
  a cursor with an entry whose `k` is 0, an entry whose `s` is above 9223372036854775807, or a text that is not exactly
  what the codec writes for its entries (whitespace, another key order, a duplicate key or other escaping).

The server emits a `cursor` line after every batch of `event` lines and after every `snapshotEnd`; its entry for a
project stands at the last position the stream sent for it. The client stores the last `cursor` line it received in
full, after it has applied every line before it. To resume it passes the text as `--cursor`; the server reads each
project's entry, asks the feed route for the events after `(s, k)` with the anchor `h`, and checks `e` against the
project's epoch (section 6). A project the cursor does not name starts with a snapshot. A cursor entry whose project
is no longer found is kept (its `r` lets discovery look at the root again) until the client drops it (section 6).

```json
{"v":1,"kind":"cursor","cursor":"c1.eyJwIjp7InByb2otYSI6eyJlIjoiZGFlY2EzYjIyNDVmMzUxOSIsImgiOiIwZjFlMmQzYzRiNWE2OTc4IiwiciI6Ii9zcnYvd29yay9hbHBoYSIsInMiOjI2fSwicHJvai1iIjp7ImQiOjE3OTAwMDAwMDAsImUiOiI1YTZiN2M4ZDllMGYxYTJiIiwiaCI6IjExMjIzMzQ0NTU2Njc3ODgiLCJrIjoxLCJyIjoiL3Nydi93b3JrL2JldGEiLCJzIjo3fX19"}
```

The same cursor, decoded (`proj-a` has `k` 0 so the field is omitted; `proj-b` is unavailable since the time in `d`):

```json
{"p":{"proj-a":{"e":"daeca3b2245f3519","h":"0f1e2d3c4b5a6978","r":"/srv/work/alpha","s":26},"proj-b":{"d":1790000000,"e":"5a6b7c8d9e0f1a2b","h":"1122334455667788","k":1,"r":"/srv/work/beta","s":7}}}
```

Positions: a ledger row `seq` with `n` public sub-events has the positions `(seq, 0)` to `(seq, n-1)`; a row with none
has the single position `(seq, 0)`. `(0, 0)` is before the ledger. `after` names the last position the caller
consumed and reading starts at the next one. The epoch is 16 hex of the SHA-256 of the event id of sequence 1 (derived
from the project id when the ledger has no sequence 1); a client that sees it change holds a replaced ledger. The anchor
is 16 hex of the SHA-256 of the event id at `s`; none exists for sequence 0.

## 4. Snapshot, then events

A client that has no state for a project builds it like this:

1. Take the snapshot head: `feed snapshot` answers `{epoch, head, anchor, run}`. `head` is `H`; `anchor` is the history
   anchor of `H` (section 3), `null` when `H` is 0.
2. Read every page of `waiting`, `agents`, `plans` and `messages` (`feed waiting|agents|plans|messages`, following
   `next` until it is `null`). A host older than `messages` refuses `feed messages` with `invalid_request`; the client
   goes on without that section. Each page is read at its own head, which may be newer than `H`. The run item comes with the snapshot head.
3. Read the events after `H`: `feed page --after H:0` and on, until `hasMore` is false.
4. Apply the events in order. An item that changed after `H` may be in a page and also in an event; the client
   de-duplicates by item key, and the latest read of an item wins.

On the stream the server does the same and sends the result: `snapshot` lines (each with `projectId`, `section`,
`key` and the item), `snapshotEnd` with `head` = `H`, then `event` lines for the sequences after `H`.

A `feed head` answer is `{projectId, epoch, head, anchor}`, where `anchor` is the history anchor of `head`, `null` when
`head` is 0.

A `feed page` answer is `{projectId, epoch, head, anchorOk, ahead, events, scanned {seq, k}, anchor, hasMore, rowsRead}`:
`scanned` is the last position examined (an event returned or a row skipped) and is where the next `--after` stands;
`anchor` is the history anchor of `scanned.seq`, read in the same call as the page, `null` when that sequence is 0 or
the ledger lacks it (it is the `--anchor` of the next page); `rowsRead` is diagnostic.

### Messages to the user

An agent sends the user a message with `cstan send [--action|--info] @user <text>` (at most 4096 bytes; the agents of a
project send at most 60 an hour, the next is refused with `limit_reached`; a caller that is not an active agent is
refused with `not_an_agent`). `--action` asks a question: it is `open` and waits in `waiting` as kind `message` until
the user answers it with `message reply` (section 11). Without `--action` it is `info`. The agent that asked closes a
question the user answered at its terminal with `cstan send --answered @user <n>` (`n` is the message's `sequence`): it
becomes `answered` and stops waiting; another agent's question is refused with `not_yours`, one that is not open with
`not_open`.

`feed messages [--after <key>] [--limit <n>]` lists the latest 200 messages to the user, oldest first. An item is
`{key, messageId, sequence, senderAgentId, class, state, body, truncated, createdAt, answeredAt, requestId}`: `class`
is `action` or `info`, `state` is `open`, `answered` or `info`, `body` is cleaned and cut at 1024 bytes (`truncated`),
and `requestId` is `message:<messageId>`, the request id of the waiting item while the message is open. The waiting
item's `summary` is the body and its `contentHash` is the SHA-256 of the canonical JSON of `{id, body}`.

Sending or closing writes a `snapshot.invalidated` event for the item in `messages` and, for a question, its item in
`waiting`. A
`decision.remote` on a `message:` request also invalidates the item in `messages`. The body is the agent's text: a
client shows it as plain text, never as a link or rich text, and never in a notification.

### Recent history

`feed recent --epoch <hex16> --head <n> [--limit <n>]` reads the newest outcome events at or below `head`, so a client
that has just connected can show what happened lately. `--epoch` and `--head` are the ones the client holds (`feed
snapshot`); `--limit` is 1 to 20 and defaults to 20. The answer is `{projectId, epoch, head, events, oldestScanned,
truncated, rowsRead, rowsScanned}`. `events` are the events of `feed page` (same ids, envelope and body) of the types
`report.recorded`, `review.verdict`, `verify.result`, `integration.finished`, `plan.approved`, `plan.signed_off`,
`design.decided`, `agent.lost`, `agent.blocked` and `finding.raised`, ordered by `(seq, k)` descending. The read goes
backwards from `head` one ledger row at a time and takes a row whole: when the limit, the budget of 500 reads
(`rowsRead`, rows and entity lookups together) or 48 KiB of events would stop inside a row, none of that row's events are
returned. `oldestScanned` is the lowest sequence taken and `rowsScanned` the rows taken; `truncated` is false only when
the read reached the first row of the ledger. `truncated` true with no events is a normal answer: the client says that
there are no outcomes in the last `rowsScanned` rows. There is no continuation: the overview shows the newest window.

An `epoch` that is not the ledger's, or a `head` beyond the ledger's, is refused with the answer `{projectId, epoch,
head}` of the ledger, and the client reads the snapshot again. A `head` older than the ledger's is allowed and reads back
from it. The client joins the answer with the live stream by `projectId`, `epoch`, `seq` and `k`: the answer holds only
rows at or below the `head` the client passed, live events above it come from the stream, and when both have a key the
live copy wins. An event is built from the current state of its entity, as `feed page` builds it, so an old event read
again may show newer values than it did when it was live.

The answer is display-only: the client shows it and applies no `effects` from it, and nothing is written. A call is
limited to 30 a minute for a project; the next fails with `rejected` and a message starting `rate_limited: `. A host
that does not know `recent` answers `invalid_request` (with its old usage text) or `unknown_command`; the client treats
both as no recent history on this host.

Item keys come from `KEY_KINDS`, in sort order:

| section | kind | key | number comes from |
|---|---|---|---|
| `waiting` | `op` | `op:<n>` | `operator_proposals.sequence` |
| `waiting` | `prompt` | `prompt:<n>` | `prompt_relays.sequence` |
| `waiting` | `design` | `design:<plan sequence>/<packageId>` | `plans.sequence/plan_packages.package_id` |
| `waiting` | `message` | `message:<n>` | `user_messages.sequence` |
| `waiting` | `integration` | `integration:<n>` | `integrations.sequence` |
| `agents` | `agent` | the agent id | `agents.agent_id` |
| `run` | `run` | `run` | none |
| `plans` | `plan` | the plan sequence | `plans.sequence` |
| `messages` | `message` | the message sequence | `user_messages.sequence` |

`<n>` is a decimal number without padding. A key that is only a kind name (`message`, `design`, `plan`) is not an item
key: it asks the client to re-read every item of that kind.

Effects: every event has `body.effects`, a list of `{section, op, key, fields}` for the sections `waiting`,
`agents`, `plans`, `messages` and `run`. `op` is:

- `upsert`: set the item `key` of `section` to include `fields`. It is used only for facts fixed when the ledger row is
  written (a registered agent); a client that holds the item merges `fields` into it. One ordered group is also
  carried: `activity`, `activitySince` and `activitySeq` (an `agent.activity` event) are fixed for that row, and a client
  applies the three together only when `activitySeq` is greater than the one it holds (none held counts as 0). A client
  ignores an upsert carrying `activitySeq` for an agent key it does not hold, with no refresh: it only adds activity to
  an item the client has. In the snapshot-then-events order no read leaves a stale activity: an item page read after an
  event already holds an `activitySeq` at least as great, and an event read before the item's page applies to an item the
  client already holds, where the greater value wins. Sequences are not contiguous.
- `remove`: delete the item `key` from `section`.
- `invalidate`: the item changed in a way the event does not state. The client reads the item again with
  `feed item <section> <key>` (a `snapshot_item`): an answer with `item` set replaces it, an answer with `item` null
  removes it. When `key` is a bare kind name the client reads the pages of that kind again.

An event never claims a value that can move afterwards: values that can move live only under `body.current`, which also
holds `readAt`, the head the read was made at, and never decide an event's type or effects.

## 5. Envelope, events and items

Every event is:

```
{id, seq, k, ts, type, actorId, entity {type, id}, from, to, body}
```

`id` is `<projectId>:<epoch>:<seq>` with `:<k>` added only for a row that lists several items. `seq` and `k` are the
position; `ts` is the time the row was written; `from` and `to` are states, or `null`. The fields fixed when the row
is written never change on replay.

`body` always has `effects` and, from a live read, `current`. Required body fields by type (a field marked `?` is
always present but may be `null`):

| type | required body fields |
|---|---|
| `message.action_needed` | `messageId`, `recipientAgentId`, `state`, `body`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `message.state` | `messageId`, `recipientAgentId`, `state`, `body`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `prompt.waiting` | `relayId`, `agentId`, `state`, `text`, `options`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `prompt.answered` | `relayId`, `agentId`, `state`, `text`, `options`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `prompt.expired` | `relayId`, `agentId`, `state`, `text`, `options`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `operator.proposed` | `proposalId`, `kind`, `state`, `command`, `decision`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `operator.decided` | `proposalId`, `kind`, `state`, `command`, `decision`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `operator.finished` | `proposalId`, `kind`, `state`, `command`, `decision`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `report.recorded` | `reportId`, `agentId`, `commitSha`, `state`, `summary`, `textVerified`, `truncated`, `untrusted` |
| `design.decided` | `planId`, `packageId`, `reportId`, `decision`, `link` |
| `plan.submitted` | `planId`, `agentId`, `revision`, `state` |
| `plan.approved` | `planId`, `agentId`, `revision`, `state` |
| `plan.signed_off` | `planId`, `integrationId`, `state` |
| `plan.changed` | `planId`, `state` |
| `plan.needs_attention` | `planId`, `agentId`, `kind`, `state`, `episodeMs` |
| `review.verdict` | `reviewId`, `verdict` |
| `verify.result` | `runId`, `result` |
| `finding.raised` | `findingId`, `raisedByAgentId`, `targetAgentId`, `severity`, `state` |
| `finding.resolved` | `findingId`, `result`, `state` |
| `agent.registered` | `agentId`, `kind`, `role`, `generation`, `paused`, `state` |
| `agent.lost` | `agentId`, `kind`, `role`, `generation`, `paused`, `state`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `agent.paused` | `agentId`, `kind`, `role`, `generation`, `paused`, `state`, `scope`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `agent.resumed` | `agentId`, `kind`, `role`, `generation`, `paused`, `state`, `scope`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `agent.ended` | `agentId`, `kind`, `role`, `generation`, `paused`, `state` |
| `agent.blocked` | `agentId`, `kind`, `role`, `generation`, `paused`, `state`, `blocker` |
| `agent.activity` | `agentId`, `activity`, `activitySince`, `activitySeq` |
| `run.changed` | `state`, `paused`, `reason`, `textVerified`, `truncated`, `untrusted` |
| `integration.finished` | `integrationId`, `state` |
| `decision.remote` | `requestId`, `outcome`, `verb`, `client`, `contentHash`, `messageId`, `replaces` |
| `snapshot.invalidated` | `section`, `key` |

Notes: `plan.needs_attention` comes from the controller's attention list, which lists agent episodes, not plans: its
`planId` is `null`, its `state` is the episode kind and its entity is an agent. `decision.remote` is the record of a decision made with `cstan decide` (section 11): `messageId` and `replaces` may be `null`,
and its one effect is an `invalidate` of the item the decision was made on (a `waiting` key, the agent id in `agents`, or
`run`). For `agent.blocked` the fields
`generation` and `paused` may be `null`; for `agent.lost` `paused` may be `null`. `snapshot.invalidated` carries the
full item key of the snapshot (`integration:3`, an agent id), which is what `feed item` takes.

An event from the kernel's golden fixtures (the placeholders `actorId` and `ts` of the fixture are filled in):

```json
{"v":1,"kind":"event","projectId":"prustonly","event":{"actorId":"a1b2c3d4-0000-4000-8000-000000000001","body":{"agentId":"dev-3","current":{"readAt":20},"effects":[{"fields":{"agentId":"dev-3","generation":1,"kind":"Developer","paused":false,"role":"developer","state":"active"},"key":"dev-3","op":"upsert","section":"agents"}],"generation":1,"kind":"Developer","paused":false,"role":"developer","state":"active"},"entity":{"id":"dev-3","type":"agent"},"from":null,"id":"prustonly:daeca3b2245f3519:20","k":0,"seq":20,"to":"active","ts":"2026-10-01T09:30:00.000Z","type":"agent.registered"}}
```

Snapshot items, as the paged reads serialize them (camelCase). A `snapshot` line carries the item in `item` and adds
`projectId`, `section` and `key`. `WaitingItem`: `key`, `requestId`, `kind`, `since`, `summary`, `contentHash`,
`needsTerminal`, `integrationState`, `branch`, `headSha`, `integrationDetails`. `integrationState` (`merged` or
`conflicted`), `branch` and `headSha` are set for kind `integration` and `null` for every other kind. `AgentItem`: `key`, `agentId`, `kind`, `role`, `generation`, `state`, `paused`, `task`, `requestId`,
`contentHash`, `branch`, `activity`, `activitySince`, `activitySeq`. `branch` is optional (absent or `null` without a
pane) and untrusted text (section 8). The three activity fields are the agent's latest observed
activity state, the time it was observed and its sequence; they are `null` while the agent has no observation, and
`contentHash` does not cover them. `PlanItem`: `key`, `planId`, `sequence`, `title`, `state`, `packages {total, assigned, reported,
integrated}`. `state` of a plan is `draft`, `in_review`, `approved` or `signed_off`: a plan with a sign-off stays in
`plans`, as `signed_off`, while one of its sign-offs is on an integration that is neither confirmed nor discarded, and
leaves when that integration is confirmed or discarded (a cancelled plan is never listed). `RunItem`: `requestId` (`run`), `state`, `paused`, `contentHash`; the run item has no key of its own and
its line uses `run`. `Page<T>`: `head`, `items`, `next`.

#### `integrationDetails`

The waiting item of an integration carries `integrationDetails`: what the integration brought and how its checks went.
It is `null` for every other kind, and `{"recorded": false}` for an integration with no stored details (one merged before
details were recorded). Otherwise it has exactly these members: `recorded` (true), `impactError`, `plans`, `plansOmitted`,
`reports`, `reportsOmitted`, `signoffs`, `signoffsOmitted`, `impact`, `review` and `verify`.

- `impactError` is `null`, or `timeout` or `diff_failed` when the impact of the diff could not be worked out (then
  `impact` is `null`; `impact` is also `null` when none was recorded).
- `plans` holds `{planId, title}`: the plan the integration's commit names. `reports` holds `{reportId, packageId,
  summary}` (`packageId` may be `null`), at most 20. `signoffs` holds `{planId, summary, createdAt}`, oldest first, at
  most 10, and `signoffsOmitted` counts the rest. A sign-off may name a plan that is not in `plans`: `plans` holds only the plan the merge
  commit's information names (the plan the integration was started for), while a sign-off is recorded per plan.
- `impact` is `{filesChanged, insertions, deletions, truncated, areas, areasOmitted, migrations, migrationsOmitted,
  nextSteps}`: `areas` holds `{path, files}` (at most 12), `migrations` holds `{path}` (added migration files, at most
  10) and `nextSteps` holds `{text}`. `truncated` is true when the diff was cut short.
- `review` is `null` or `{verdict, reviewerAgentId, round}`: the finished review of the integration with the greatest
  round; `verdict` is `passed`, `findings`, `failed` or `cancelled`. A review that has started but not finished is not
  shown.
- `verify` is `null` or `{latest, lastPassed}`. `latest` is `{state, commitSha, finishedAt}` of the finished run with the
  greatest sequence among the runs of the integration that ended `passed`, `failed`, `timeout` or `error`; `lastPassed` is
  `{commitSha, finishedAt}` of the latest run that passed, or `null`. A later cancelled or interrupted run hides nothing,
  and queued and running runs are not shown. Runs of a report on the same commit are not the integration's runs.

Every text value (`title`, `summary`, `path`, `text`) is an object that holds the text and carries `textVerified` and
`truncated` as members beside it (the `migrations` and `nextSteps` entries too). Texts are cleaned and cut to 300 bytes. They
are read from rows that cannot be changed once written, so no writer hash covers them: `textVerified` is true for every
one of them, and the flag stays so a client treats every text from outside the same way. They are untrusted text
(section 8). The stored text says only that some migration path was cut, so `truncated` is set on a migration path
that reaches the cap when one was.

`integrationDetails` is at most 17 KiB serialized (the stored text is at most 16 KiB; entries are left out from the end of
a list, and counted in its `Omitted` member, when the flags make it longer), so an item stays within the line and page
limits (section 10).

`contentHash` does not cover `integrationDetails`. The details change while the hash does not (a review verdict, a
verify result or a sign-off), so a client replaces an item wholesale each time it reads it and never skips a read
because the `contentHash` is unchanged. A `review.verdict`, `verify.result` or `plan.signed_off` event of an integration
that is still `merged` or `conflicted` carries an `invalidate` of its waiting item; those events for a report carry none.

One page of each section (the values are examples; the hashes are the SHA-256 of the canonical JSON the item's hash is
defined over):

```json
{"head":26,"items":[{"key":"message:3","requestId":"message:msg-1","kind":"message","since":"2026-10-01T09:30:00.000Z","summary":"Please look at plan 1","contentHash":"fa78fb90bf4be31537ca520c46cd57bf447d8aea8b51d2b614649c3f37fa2639","needsTerminal":false,"integrationState":null,"branch":null,"headSha":null,"integrationDetails":null},{"key":"integration:7","requestId":"integration:int-7","kind":"integration","since":"2026-10-01T09:30:00.000Z","summary":"merged integration/plan-3","contentHash":"6741f9c2efe51328a0059c87de7b9ab66af0489c005fca8ec2f46d2eeb96fbd6","needsTerminal":false,"integrationState":"merged","branch":"integration/plan-3","headSha":"3f2a9c1d5b7e4a60918c2d4e6f8091a2b3c4d5e6","integrationDetails":{"recorded":true,"impactError":null,"plans":[{"planId":"plan-3","title":"Remote protocol","textVerified":true,"truncated":false}],"plansOmitted":0,"reports":[{"reportId":"rep-9","packageId":"pkg-a","summary":"Added the feed","textVerified":true,"truncated":false}],"reportsOmitted":0,"signoffs":[{"planId":"plan-3","summary":"Reviewed and signed off","createdAt":"2026-10-01T10:00:00.000Z","textVerified":true,"truncated":false}],"signoffsOmitted":0,"impact":{"filesChanged":12,"insertions":340,"deletions":25,"truncated":false,"areas":[{"path":"rust/crates/kernel","files":9,"textVerified":true,"truncated":false}],"areasOmitted":0,"migrations":[{"path":"migrations/0053_integration_details.sql","textVerified":true,"truncated":false}],"migrationsOmitted":0,"nextSteps":[{"text":"Run the migrations","textVerified":true,"truncated":false}]},"review":{"verdict":"passed","reviewerAgentId":"reviewer-2","round":2},"verify":{"latest":{"state":"passed","commitSha":"3f2a9c1d5b7e4a60918c2d4e6f8091a2b3c4d5e6","finishedAt":"2026-10-01T09:50:00.000Z"},"lastPassed":{"commitSha":"3f2a9c1d5b7e4a60918c2d4e6f8091a2b3c4d5e6","finishedAt":"2026-10-01T09:50:00.000Z"}}}}],"next":null}
```

```json
{"head":26,"items":[{"key":"dev-1","agentId":"dev-1","kind":"Developer","role":"developer","generation":1,"state":"active","paused":false,"task":null,"requestId":"agent:dev-1","contentHash":"b4f045b6e6f57de6c9bcf11373ce42ab0959a698e99d40293413fd13403ce3f0","activity":"working","activitySince":"2026-10-01T09:30:00.000Z","activitySeq":1}],"next":null}
```

```json
{"head":26,"items":[{"key":"1","planId":"plan-1","sequence":1,"title":"Remote protocol","state":"approved","packages":{"total":3,"assigned":2,"reported":1,"integrated":0}}],"next":null}
```

```json
{"requestId":"run","state":"active","paused":false,"contentHash":"b47b658cf5dd643526252edbaca2e5ef54b936794d728ddb6193be0f194dcf9a"}
```

## 6. Errors and resync

Error codes. The `feed` route (operator only) answers `unauthorized` (the key is not accepted), `forbidden` (the
caller is an agent or unknown), `unknown_command` (the daemon does not know `feed`), `invalid_request` (a bad
subcommand, flag or value; the message names it) and `error` (the command failed; nothing is shown of the cause). The
stream adds `bad_cursor` (the `--cursor` text does not decode and the server could not continue) and `internal`; both
end the stream with exit code 1. The codes of `cstan decide` are in section 11.

Resync rules, per project. The client drops what it holds of the project and reads the snapshot again when:

- the epoch of the project is not the `e` of its cursor entry: the ledger was replaced (the server sends `resync`
  with reason `epoch`);
- a page answers `anchorOk` false: the history at `s` is not the one the client saw (reason `anchor`);
- a page answers `ahead` true (it also has `anchorOk` false): the cursor stands beyond the head, the ledger was
  restored to an earlier state (reason `ahead`);
- an event has an effect with a section or an op the client does not know (reason `effect`, sent by the client's own
  logic; there is no line for it);
- the project is `unsupported`, `silent` or `gone`: no resync. The server sets `d` in the entry (when it is not set),
  keeps the entry in the cursor and sends the `project` line with its state. When the project is available again
  it clears `d` and continues from the entry; the epoch and anchor checks above then decide whether a resync follows.
  The client may drop an entry whose `d` is older than a time the client chooses. `no_key` and `refused` projects keep
  their entry without `d`; they need action on the host.
- the cursor does not decode: the server writes a `resync` line with `projectId` `null` and reason `cursor`, then
  sends a snapshot of every project as if no cursor was given.

`resync` reasons are `epoch`, `anchor`, `ahead`, `effect` and `cursor`. After a `resync` for a project the server sends
the snapshot of the project before any further event of it.

## 7. Version and compatibility

`v` is the major version of the framing and the events. In v1:

- new fields may be added to any line, event body or item, and new event types may appear. A client ignores what it does
  not know, except `body.effects`: the effects of an event of an unknown type are applied.
- an effect with an unknown `section` or `op` makes the client resync that project.
- a line with a `v` the client does not know is refused: the client stops and tells the person.

A field is never removed or changed in meaning within a major version.

## 8. Untrusted text

Text in events and items comes from agents, messages and commands and is not to be trusted.

- `textVerified`: the text of the entity row was hashed again the way its writer hashed it and compared with the hash in
  the ledger. When it is false the text is left out (empty or `null`) and the client must not show a stand-in.
- `untrusted`: the body paths that hold text from outside (`["body.text","body.options"]`); show them as untrusted.
- `truncated`: the text was cut to its cap.
- `clean_text` is a processing step, not a field: the server replaces control, format and line and paragraph separator
  characters by a space in text it shows, before the text is capped.
- Caps: event text 4096 bytes, a reason or note 1024 bytes, a prompt carries at most 32 options of at most 256 bytes
  each, a snapshot item's text (`summary`, `task`, `title`) 1024 bytes.

A client renders every such field as plain text: no markup, no links followed, no escape sequences interpreted. It never
runs or pastes a command, path or link from them without the person reading it.

| cap | value |
|---|---|
| `event_feed::TEXT_CAP` | 4096 |
| `event_feed::REASON_CAP` | 1024 |
| `event_feed::OPTIONS_CAP` | 32 |
| `event_feed::OPTION_TEXT_CAP` | 256 |
| `snapshot_feed::TEXT_CAP` | 1024 |

## 9. Security

- The operator key stays on the host. The stream command reads it there and uses it to ask the local controllers; it is
  never written to the stream, to standard error or to a cursor.
- The stream is read only: it writes nothing to a ledger and starts, stops and signals nothing.
- A notification on a phone opens the app, and the app shows the item after it has read it from the host. A
  notification carries no decision and no text from an agent.
- Decisions are made only with `cstan decide` (section 11), never on the stream. A decision is bound to the request id of
  the item, its content hash and a request nonce, so a decision on a stale or changed item is refused. A resumed pause may
  give an item a hash it had before, which is why the nonce is part of the binding.
- Access to the stream is access to everything the feed carries (messages, prompts and commands of every agent): it is
  as strong as the SSH login that starts it.

## 10. Limits

| limit | value |
|---|---|
| `event_feed::MAX_LIMIT` (events per page) | 500 |
| `snapshot_feed::MAX_LIMIT` (items per page) | 100 |
| `event_feed::MAX_SCAN_ROWS` (row reads per events page) | 2000 |
| `snapshot_feed::MAX_READS` (row reads per items page) | 2000 |
| serialized bytes per page, events and items | 524288 |
| bytes of one event | 49152 |
| bytes of one stream line | 65536 |
| minimum poll interval | 1000 ms |

A page may hold no item and still have a `next`, and an events page may be cut by a budget with `hasMore` true: the
client keeps reading. The first event of a page is always returned. A poll interval below the minimum is raised to it.

The daemon route `feed` takes these subcommands: `head`, `page --after <seq>:<k> --limit <n> [--anchor <hex16>]`,
`snapshot`, `waiting --limit <n> [--after <key>] [--kind <kind>]`, `agents --limit <n> [--after <key>]`,
`plans --limit <n> [--after <key>]`, `item <section> <key>`, `hash <requestId>` and `recent --epoch <hex16> --head <n> [--limit <n>]`.

The `head`, `snapshot` and `page` answers carry `anchor` (section 4).

`hash` always answers the object `{requestId, contentHash}`: the content hash of a request id (`op:<id>`,
`prompt:<id>`, `design:<plan>/<package>`, `message:<id>`, `integration:<id>`, `agent:<id>` or `run`), with
`contentHash` `null` once nothing waits on it. `item` always answers `{section, key, item}`, with `item` `null` when
the row is gone or no longer listed (the client removes it).

```json
{"requestId":"message:msg-1","contentHash":"fa78fb90bf4be31537ca520c46cd57bf447d8aea8b51d2b614649c3f37fa2639"}
```

```json
{"requestId":"message:msg-9","contentHash":null}
```

```json
{"section":"agents","key":"dev-1","item":{"key":"dev-1","agentId":"dev-1","kind":"Developer","role":"developer","generation":1,"state":"active","paused":false,"task":null,"requestId":"agent:dev-1","contentHash":"b4f045b6e6f57de6c9bcf11373ce42ab0959a698e99d40293413fd13403ce3f0","activity":"working","activitySince":"2026-10-01T09:30:00.000Z","activitySeq":1}}
```

```json
{"section":"waiting","key":"message:9","item":null}
```

A `page` with `--after 0:0` takes no `--anchor`: the route refuses it with `invalid_request`.

Discovery: a controller is looked for among the running controller processes of the current user and the roots given
with `--root` or found in a cursor. A project whose controller never ran and that is not named is not listed. One
probe waits at most 2000 ms for an answer. The stream follows at most 64 projects; more is an `error` with code
`invalid_request` before the first line.

## 11. Decisions

A client acts on a waiting item with one command, `cstan decide`. It is not part of the stream: the stream stays read
only (section 9), and every decision is a separate SSH call.

### Transport

```
ssh <host> cstan decide ...
```

Synopsis: `decide --project <root|projectId> [--json] <requestId> --expect <hash> --nonce <n> [--client <label>] <verb> [<args>]`

`--project` and `--json` come before the request id. A `--project` value that starts with `/` is a project root; any
other value is a project id. The command works from any folder, uses the operator key of the project on the host and
never starts a controller. With `--json` it writes one JSON object and a newline to standard output; without it, one
readable line (`<requestId> <verb>: <outcome>`, then `(repeat)` and `, message <id>` when there are any). Standard
error is for a person; a client does not parse it.

### Binding

A decision names the item it is about and the state of the item the person saw:

- `<requestId>` is the `requestId` of the item (`op:…`, `prompt:…`, `design:…`, `message:…`, `integration:…`, `agent:…`
  or `run`), or `pm` for a message to the project's PM (see Message to the PM).
  Other kinds are refused with `invalid_request`.
- `--expect <hash>` is the `contentHash` of the item, as the snapshot or `feed hash` gave it: 12 to 64 lowercase hex
  characters, a prefix of the full hash. A hash that is not the current one is refused with `hash_mismatch`.
- `--nonce <n>` is chosen by the client for each decision: 8 to 64 characters of letters, digits, `_` and `-`. A resumed
  pause may give an item a hash it had before, so the hash alone cannot tell a new decision from an old one.
- `--client <label>` names the client (1 to 64 characters of letters, digits, space, `.`, `_` and `-`; default `remote`).
  It is recorded and shown to the person who confirms a forward; it is text from the client and not to be trusted.

### Verbs

One row per accepted `(kind, verb)`; `kind` is the part of the request id before `:`, and `applied` and `forwarded` are
the two outcomes.

| kind | verb | outcome |
|---|---|---|
| op | deny | applied |
| op | approve | forwarded |
| prompt | option | forwarded |
| prompt | esc | forwarded |
| design | approve | applied |
| design | changes | applied |
| message | reply | applied |
| agent | pause | applied |
| agent | resume | applied |
| run | pause | applied |
| run | resume | applied |
| integration | merge | applied |
| pm | message | applied |

Arguments: `op deny [<note>]`; `prompt option <n>` (a whole number of at least 1, an option of the prompt as shown);
`design approve --link <url> --asked-user <text>`; `design changes --asked-user <text> [--link <url>]`;
`message reply <text>`; `agent pause|resume [--reason <text>]`; `run pause|resume [--reason <text>]`; `integration merge`
(no arguments; see Merge); `pm message <text>`. Text is cleaned
and capped like the text of the feed (section 8).

An `applied` verb is made by the controller as the operator, at once. A verb is `forwarded` when it would approve a
proposal or answer a prompt of an agent: a tap on a phone is not an approval. The controller sends the active PM a
message instead. The message names the request, the verb and the exact command to run, and tells the PM to confirm with
the person first; the text the client supplied (its label, the label of an option) comes after a heading that marks it
as untrusted. Nothing is typed into any terminal and nothing is approved until the PM has the person's answer. A second
forward of the same request and hash replaces the first: the result's `replaces` names the earlier message, which the
PM is told not to act on, and an applied decision on the request ends the forwards still open. One request takes at most
10 messages to the PM from remote clients (forwards and replies); the next is refused with `limit_reached`.

Some answers are not made remotely and are refused with `needs_terminal`: a text answer to a prompt, an option that
widens permissions or accepts typed text, a prompt with no option that can be answered remotely, a `--session` or
`--window` grant on `op approve` or on a prompt.

A `message reply` answers an open question an agent sent the user (section 4, Messages to the user). The answer goes to
that agent as an action message with the text marked as untrusted, quoting the question (cut at 600 bytes), and the
question becomes `answered`. A question whose agent is no longer active is refused with `not_waiting`.

### Merge

`integration merge` merges a waiting integration (`integration:<id>`, `integrationState` `merged`) into the project's
branch, as the daemon of the project and never through a terminal. It is `applied`. It is off unless the project's
configuration allows it (`[remote] merge = true`, with `merge_branch` naming the branch or the remote's default branch);
it never pushes. The gates answer in this order, each with its refusal:

| code | meaning |
|---|---|
| `merge_disabled` | merging from a remote client is not enabled for this project |
| `run_paused` | the project's run is paused |
| `busy` | an integration is running |
| `not_reviewed` | the integration's latest review has not passed |
| `not_verified` | the integration head has no passed verify run under the current commands |
| `branch_moved` | the integration branch moved since the integration was made |
| `invalid_commit` | the commit to merge is not a valid commit |
| `invalid_branch` | the branch is not a valid branch name |
| `unsafe_config` | the repository's git configuration is not safe for a merge |
| `wrong_branch` | the project's checkout is not on the branch the merge goes into |
| `no_identity` | git has no committer identity in the repository |
| `dirty_tree` | the working tree has uncommitted changes to tracked files |
| `git_too_old` | git is older than the merge needs |
| `merge_conflict` | the integration does not merge cleanly; the message names the files (at most a few, then `and N more`) |
| `merge_failed` | the merge started and did not end well; the message gives the reason and whether the tree was restored |
| `git_unavailable` | git could not be run |
| `merge_unrecorded` | the merge was made but its decision could not be recorded; the same call again records it |

A merge that was made adds `merge` to the result: the integration and branch, the merge commit (`null` when the head was
already in the project's branch, `alreadyInHead` true), the verify state (`passed` with its run, or `not_configured`),
the cleanup of the merged branch (`scheduled`, or `null` when cleanup is off) and `push`, which is always `disabled`.
The member as it appears in the result (the other fields are those of Result below):

```
{"merge":{"integrationId":"int-7","branch":"integration/plan-3","mergeCommit":"9d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e","alreadyInHead":false,"verify":{"state":"passed","runId":"verify-12"},"cleanup":{"state":"scheduled"},"push":{"state":"disabled","reason":"push_off"}}}
```

The merge also confirms the integration and tells the PM. A merge may take minutes; a client waits up to five.

### Message to the PM

`pm message <text>` sends the project's active PM a message from the person, at once (`applied`). The PM gets it as an
action message whose text is marked as untrusted, from the client named by `--client`; the result's `messageId` names
it, and its delivery shows in the feed like any message to the PM. `--expect` binds the message to the PM the person
saw: it is the SHA-256, in lowercase hex, of the canonical JSON `{"pm":"<agentId>"}` of the one agent of kind `PM` in
state `active` (for `pm-1`, the SHA-256 of `{"pm":"pm-1"}`). With no active PM the call is refused with `no_active_pm`;
once another PM has taken over, with `hash_mismatch`. Messages to the PM are not capped per request but count toward
the hourly limit of the project (`[remote] messages_per_hour`, 30 by default), with forwards and replies.

### Result

A decision that was made, or repeated, answers one object with these fields (`messageId` and `replaces` are present only
when there is a message or an earlier forward):

```json
{"v":1,"decisionId":"remote-0000000004","requestId":"op:proposal-2","nonce":"n-7f3a91c2","outcome":"forwarded","repeat":false,"kind":"op","verb":"approve","contentHash":"b47b658cf5dd643526252edbaca2e5ef54b936794d728ddb6193be0f194dcf9a","eventSeq":57,"messageId":"3f0c2a9e-5d4b-4c1e-9a77-0e8d6b1f2c34","replaces":"8a1d5e72-93c0-4b6f-a2d1-5c7e90b4f3a8"}
```

`eventSeq` is the `seq` of the `decision.remote` event (section 5) the decision wrote. `contentHash` is the full hash the
decision was checked against.

### Refusal

A refusal answers `{v, error {code, message}}` on standard output with `--json`, and `cstan decide: <code>: <message>` on
standard error without it. `message` is for a person.

```json
{"v":1,"error":{"code":"hash_mismatch","message":"op:proposal-2 has changed since it was shown"}}
```

| code | meaning |
|---|---|
| `not_waiting` | the request is not waiting for a decision (decided already, or gone) |
| `hash_mismatch` | `--expect` is not a prefix of the current content hash |
| `nonce_reused` | the nonce was used for another decision |
| `needs_terminal` | the answer is made at the terminal (see Verbs) |
| `no_active_pm` | a forward, a reply or a message to the PM needs an active PM and there is none |
| `limit_reached` | the request already has 10 messages to the PM from remote clients, or the project sent its hourly number of messages to the PM |
| `invalid_request` | the request id, a flag, the verb or its arguments are not valid |
| `merge_…`, `busy`, … | the refusals of `integration merge` (see Merge) |
| `forbidden` | the caller is not the operator |
| `unauthorized` | the operator key is not accepted |
| `project_not_found` | `--project` names no project (no folder with a project, no running controller with that id) |
| `no_operator_key` | the project has no usable operator key |
| `not_answering` | the controller of the project did not answer in time |

### Idempotency

The nonce makes a retry safe. A call whose nonce already has a decision, with the same request id, verb, arguments, client
and an `--expect` that is a prefix of the stored hash, answers the stored result with `repeat` true and changes nothing:
no second message is sent and no second event is written. The same nonce with anything else is `nonce_reused`. A client
that lost an answer repeats the call with the same nonce; it takes a new nonce for a new decision, even for the same item.

### Exit codes

`0` the decision was made or repeated; `1` the decision was refused (the refusal is written as above); `2` the command
line was invalid (nothing is sent); `255` is SSH's own failure.
