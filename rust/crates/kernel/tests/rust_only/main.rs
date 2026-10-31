//! Rust-only tests of kernel behaviour the frozen Node corpus never recorded (rows the retired test map listed as not
//! ported): the restart summary's budget and truncation rules (test/panes.test.ts) and a ledger that lacks migration 0030
//! (test/prompt-relay-ledger.test.ts). They state the behaviour as the Node tests described it and build every state through
//! the kernel, with the database edited only where a Node test edited it (raw SQL on the closed ledger).

mod approval_windows;
mod controller_relay;
mod design_approvals;
mod event_feed;
mod feed_cursor;
mod fixture;
mod id_prefix;
mod integration_settle;
mod message_classes;
mod plan_amend;
mod plan_reassign;
mod plan_show_cost;
mod prompt_relay_backfill;
mod remote_decisions;
mod remote_merge;
mod restart_summary;
mod role_limit;
mod snapshot_feed;
mod timed_grants;
mod user_messages;
mod verify_runs;
mod warm_reviewers;
mod write_once;
