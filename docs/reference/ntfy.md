# Push notifications (ntfy)

Capstan can push a short message to your phone or desktop through an [ntfy](https://ntfy.sh) server when something needs a person: a question, a proposal, a stalled or lost agent, a failed verify run. This page is the setup guide. The keys of the `[ntfy]` section are in [Configuration](configuration.md#push-notifications-ntfy); `cstan notify test` is in [Commands](commands.md).

The controller sends the messages by running `curl`. Nothing is sent unless `[ntfy]` has `enabled = true`.

## Set it up

1. **Make a publish-only token.** On your ntfy server, deny anonymous access (`auth-default-access: deny-all`), make a user for Capstan, give it write-only access to one topic, and create an access token for it. Capstan only publishes; it never needs to read the topic.
2. **Give the controller the token.** Export the variable named by `token_env` in the shell that starts the controller, or put the token in a file named by `token_file` (an absolute path, a regular file you own, mode 0600: `chmod 600`). Do this before `cstan start`: the controller reads its environment once, when it starts. If the server uses a private certificate authority, name its PEM bundle in `ca_file`. A proxy is taken from `HTTPS_PROXY` (or `HTTP_PROXY` for an `http` server) and `NO_PROXY` of the controller's environment.
3. **Add the section** to capstan.toml:

   ```toml
   [ntfy]
   enabled = true
   server = "https://ntfy.example.org"
   topic = "my-project"
   token_env = "NTFY_TOKEN"
   ```

4. **Restart the controller** (`cstan restart`). Changes to `[ntfy]` take effect only after a restart.
5. **Send a test:** `cstan notify test`. It prints `ntfy: test message sent (HTTP 200)` and exits 0, or `ntfy: <reason>: <error>` and exits 1. `cstan notify test --json` prints the controller's answer.
6. **Subscribe** to the topic in the ntfy app or web page of your server.

`cstan status` shows one line once the section is on: `ntfy: on -> ntfy.example.org, sent 12, failed 1, dropped 0 (last error 10:42 unauthorized)`. Without a running controller the line comes from the file the controller last wrote, and ends with `(stale: controller not running)`.

The secret is read again for every send, so you can rotate a `token_file` without a restart. It is never passed to agents, never printed, and never written to the ledger or the status file. The token and everything the controller can read are visible to the user the controller runs as; the project's agents run as that user too, so treat the token as readable by them (use a publish-only token for this reason).

## Events

| Event | Sent when |
| --- | --- |
| `user_question` | an agent asks you a question |
| `operator_proposal` | the operator proposes a command |
| `prompt_waiting` | an agent waits at a prompt |
| `design_waiting` | a design waits for approval |
| `plan_signed_off` | a plan is signed off |
| `agent_blocked` | an agent reports it is blocked |
| `agent_stalled` | an agent has made no progress |
| `agent_lost` | an agent is gone |
| `verify_failed` | a verify run failed |
| `integration_conflicted` | an integration hit a conflict |
| `pm_stuck` | the PM has waited too long (unread mail, no progress or a prompt; see `pm_stuck_minutes`) |

`events` selects which of these notify (all by default, `pm_stuck` included). An explicit list notifies only for the events it names, so add `pm_stuck` to a list written before this event existed. The message names the project (in the title prefix), the PM and the reason with the minutes, nothing else, and is sent with high priority. One stuck episode is pushed once; the same PM becoming stuck again within 30 minutes of an alerting episode raises the event for the feed but is not pushed. Priorities, tags and click links per event are in [Configuration](configuration.md#push-notifications-ntfy). The `test` message of `cstan notify test` is always allowed and is not an event you can select.

## Requirements

`curl` must be on the controller's `PATH`, version 7.68 or later, built with https support. Honouring a server's `Retry-After` after a rate limit (HTTP 429) needs curl 7.84 or later (the `%header` write-out); an older curl works but falls back to the default backoff.

Maintainers who run the end-to-end test of this feature (`cargo test -p cstan-front --test notify`) on a host that must run it set `CAPSTAN_REQUIRE_CURL=1`; without it the test prints `skipped: curl not found` and passes when curl is missing.

## Troubleshooting

`cstan notify test` and the `last error` of `cstan status` name a reason:

| Reason | Meaning and what to do |
| --- | --- |
| `not_configured` | `[ntfy]` is absent or `enabled = false`, or the controller was not restarted after you changed it. |
| `secret_missing` | The variable named by `token_env` or `password_env` is not set in the controller's environment, or the `token_file` or `password_file` does not exist. The output names the variable or path. Export it and restart the controller. |
| `secret_unreadable` | The file exists but cannot be used: wrong owner, mode other than 0600, empty or too large. |
| `unauthorized` | HTTP 401: the server refused the token or password. |
| `forbidden` | HTTP 403: the user may not write to this topic. |
| `connect_error` | The server could not be reached: check `server`, DNS, the firewall and the proxy variables. |
| `timeout` | No answer within `timeout_seconds`. |
| `transport_unavailable` | curl could not do the request, for example it has no https support or `ca_file` is not a usable bundle. |
| `transport_error` | Any other curl failure. |
| `http_429` | The server rate-limits you. Capstan waits and tries again. |
| `http_4xx`, `http_3xx`, `http_5xx` | Another HTTP answer; the error text shows the status. |
| `busy` | A test notification is already running; wait for it. |

## In the car

The ntfy iOS app marks a message Time Sensitive by its priority. Its notification service sets the interruption level from the message priority: 4 (`high`) and 5 (`urgent`) give Time Sensitive, 3 (`default`) gives the normal level, and 1 and 2 give a quiet level. Priority 5 becomes Critical only when the person turned on critical alerts in the ntfy app and iOS granted them; otherwise it is Time Sensitive.

So a Capstan event sent with `high` or `urgent` priority is a Time Sensitive notification in the ntfy app. Most events already default to `high` (see [Configuration](configuration.md#push-notifications-ntfy)); `design_waiting`, `plan_signed_off`, `agent_stalled` and `verify_failed` default to `default`, so set `priority = "high"` on them in `[ntfy.event.<name>]` to have them announced. Time Sensitive here is the ntfy app's own setting: it needs the Time Sensitive switch of the ntfy app in iOS Settings, and Announce Notifications for ntfy in CarPlay settings. This is separate from Capstan for iPhone's own switches ([car](../../apps/ios/docs/car.md)). Whether a particular car reads it aloud is not verified, see the car pass.

### Sources

- https://github.com/binwiederhier/ntfy-ios/blob/main/ntfy/Utils/NotificationContent.swift (the priority to interruption level switch, lines 43 to 68), accessed 2026-11-06
- https://github.com/binwiederhier/ntfy/blob/main/server/server_firebase.go (the server sends the iOS message with mutable-content, so the app's notification service runs), accessed 2026-11-06
- https://docs.ntfy.sh/publish/ (priorities 1 to 5), accessed 2026-11-06

## Limits

- There is no replay of downtime: events that happen while the controller is not running are not sent when it starts again.
- An agent that was stalled before a restart is not pushed again after it.
- Messages are queued in memory (a small queue per priority), coalesced when the same event repeats within a minute, and capped per ten minutes. A message that does not fit is dropped and counted in `dropped`.
- A failed send is retried `retries` times with a growing wait, then counted in `failed`.
- Changes to `[ntfy]` need a restart of the controller (`cstan restart`).
