//! `FeedCall::args` for every variant, and `call_feed` on a hand-made socket (the request frame it sends).
use capstan_client::{call_feed, FeedCall, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

fn args(call: FeedCall) -> Vec<String> {
    call.args()
}

fn text(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn args_are_the_exact_argument_lists() {
    assert_eq!(args(FeedCall::Head), text(&["head"]));
    assert_eq!(args(FeedCall::Snapshot), text(&["snapshot"]));
    assert_eq!(
        args(FeedCall::Page {
            after_seq: 7,
            after_k: 2,
            limit: 500,
            anchor: None
        }),
        text(&["page", "--after", "7:2", "--limit", "500"])
    );
    assert_eq!(
        args(FeedCall::Page {
            after_seq: 7,
            after_k: 0,
            limit: 5,
            anchor: Some("0123456789abcdef".into())
        }),
        text(&[
            "page",
            "--after",
            "7:0",
            "--limit",
            "5",
            "--anchor",
            "0123456789abcdef"
        ])
    );
    assert_eq!(
        args(FeedCall::Waiting {
            after: None,
            limit: 100,
            kind: None
        }),
        text(&["waiting", "--limit", "100"])
    );
    assert_eq!(
        args(FeedCall::Waiting {
            after: Some("k".into()),
            limit: 1,
            kind: Some("op".into())
        }),
        text(&["waiting", "--limit", "1", "--after", "k", "--kind", "op"])
    );
    assert_eq!(
        args(FeedCall::Agents {
            after: None,
            limit: 9
        }),
        text(&["agents", "--limit", "9"])
    );
    assert_eq!(
        args(FeedCall::Agents {
            after: Some("a".into()),
            limit: 9
        }),
        text(&["agents", "--limit", "9", "--after", "a"])
    );
    assert_eq!(
        args(FeedCall::Plans {
            after: None,
            limit: 3
        }),
        text(&["plans", "--limit", "3"])
    );
    assert_eq!(
        args(FeedCall::Plans {
            after: Some("p".into()),
            limit: 3
        }),
        text(&["plans", "--limit", "3", "--after", "p"])
    );
    assert_eq!(
        args(FeedCall::Item {
            section: "run".into(),
            key: "run".into()
        }),
        text(&["item", "run", "run"])
    );
    assert_eq!(
        args(FeedCall::Hash {
            request_id: "op:1".into()
        }),
        text(&["hash", "op:1"])
    );
}

#[test]
fn call_feed_sends_the_feed_command_and_returns_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for answer in [
            r#"{"ok":true,"result":{"head":3}}"#,
            r#"{"ok":false,"code":"invalid_request","message":"limit must be between 1 and 100"}"#,
        ] {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            seen.push(line);
            (&stream)
                .write_all(format!("{answer}\n").as_bytes())
                .unwrap();
        }
        seen
    });
    assert!(matches!(
        call_feed(&socket, "key", &FeedCall::Head, 5_000).unwrap(),
        Response::Ok { .. }
    ));
    match call_feed(
        &socket,
        "key",
        &FeedCall::Agents {
            after: None,
            limit: 0,
        },
        5_000,
    )
    .unwrap()
    {
        Response::Refused { code, .. } => assert_eq!(code.to_utf8_lossy(), "invalid_request"),
        other => panic!("{other:?}"),
    }
    let seen = server.join().unwrap();
    assert!(seen[0].contains(r#""command":"feed""#), "{}", seen[0]);
    assert!(seen[0].contains(r#""credential":"key""#), "{}", seen[0]);
    assert!(seen[0].contains(r#""args":["head"]"#), "{}", seen[0]);
    assert!(
        seen[1].contains(r#""args":["agents","--limit","0"]"#),
        "{}",
        seen[1]
    );
}
