mod common;
use capstan_ledger::{
    checksum, iso_from_millis, max_embedded_migration, migrations, open_database,
    open_database_read_only, prune_migration_backups, resolve_database_path, OpenOptions,
};
use common::{ledger_at, parity_file, scratch};
use rusqlite::Connection;
use std::fs;
use std::path::Path;

fn backups(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("controller.sqlite.pre-v"))
        .collect();
    names.sort();
    names
}

#[test]
fn embeds_every_file_of_the_migrations_directory() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migrations");
    let mut files: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sql"))
        .collect();
    files.sort();
    let embedded: Vec<&str> = migrations().iter().map(|m| m.name).collect();
    assert_eq!(embedded, files);
    assert_eq!(max_embedded_migration(), files.len() as i64);
    for (i, m) in migrations().iter().enumerate() {
        assert_eq!(m.version, i as i64 + 1);
        assert_eq!(m.checksum.len(), 64);
        assert!(m
            .checksum
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(m.checksum, checksum(m.bytes));
    }
    println!("{} embedded migrations", migrations().len());
}

#[test]
fn fresh_database_gets_every_migration_and_a_second_open_changes_nothing() {
    let (dir, file) = scratch();
    let db = open_database(&file, &OpenOptions::default()).unwrap();
    let ledger: Vec<(i64, String, String, String)> = db
        .connection()
        .prepare(
            "SELECT version, name, checksum, applied_at FROM schema_migrations ORDER BY version",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(ledger.len(), migrations().len());
    for ((version, name, sum, applied_at), m) in ledger.iter().zip(migrations()) {
        assert_eq!(
            (*version, name.as_str(), sum),
            (m.version, m.name, &m.checksum)
        );
        assert_eq!(applied_at.len(), 24);
        assert!(
            applied_at.ends_with('Z')
                && applied_at.as_bytes()[10] == b'T'
                && applied_at.as_bytes()[19] == b'.'
        );
    }
    let mode = db.pragma("journal_mode").unwrap();
    assert_eq!(format!("{:?}", mode[0][0]), "Text(\"wal\")");
    assert_eq!(
        format!("{:?}", db.pragma("synchronous").unwrap()[0][0]),
        "Integer(2)"
    );
    assert_eq!(
        format!("{:?}", db.pragma("foreign_keys").unwrap()[0][0]),
        "Integer(1)"
    );
    assert_eq!(
        format!("{:?}", db.pragma("busy_timeout").unwrap()[0][0]),
        "Integer(5000)"
    );
    db.close().unwrap();
    // Like Node, steps 2.. back up the ledger so far (none for step 1); only the newest three stay.
    let after_first = backups(dir.path());
    assert_eq!(after_first.len(), 3, "{after_first:?}");
    // A second open is a no-op: no migration, no new backup.
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    assert_eq!(backups(dir.path()), after_first);
}

#[test]
fn upgrade_backs_up_before_each_step_and_keeps_the_newest_three() {
    let (dir, file) = scratch();
    ledger_at(&file, 5);
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let kept = backups(dir.path());
    let last = migrations().len();
    let expected: Vec<String> = (last - 2..=last)
        .map(|v| format!("controller.sqlite.pre-v{v}-"))
        .collect();
    assert_eq!(kept.len(), 3, "{kept:?}");
    for (name, prefix) in kept.iter().zip(&expected) {
        assert!(
            name.starts_with(prefix) && name.ends_with(".sqlite"),
            "{name}"
        );
        let at = &name[prefix.len()..name.len() - ".sqlite".len()];
        assert!(
            at.len() >= 13 && at.bytes().all(|b| b.is_ascii_digit()),
            "{name}"
        );
    }
    // The backup holds the schema as it was just before that step.
    let conn = Connection::open_with_flags(
        dir.path().join(&kept[0]),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, last as i64 - 3);
}

#[test]
fn keep_option_and_prune_error_handler() {
    let (dir, file) = scratch();
    ledger_at(&file, last_minus(3));
    let options = OpenOptions {
        keep_migration_backups: 1,
        on_prune_error: Some(Box::new(|m| panic!("{m}"))),
    };
    open_database(&file, &options).unwrap().close().unwrap();
    assert_eq!(backups(dir.path()).len(), 1);
}

fn last_minus(n: usize) -> usize {
    migrations().len() - n
}

#[test]
fn prune_orders_by_version_then_timestamp_and_touches_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("controller.sqlite");
    for name in [
        "controller.sqlite.pre-v2-100.sqlite",
        "controller.sqlite.pre-v10-5.sqlite",
        "controller.sqlite.pre-v10-7.sqlite",
        "controller.sqlite.pre-v3-999.sqlite",
        "controller.sqlite.pre-v3-1.sqlite.bak",
        "controller.sqlite",
        "other.sqlite.pre-v1-1.sqlite",
    ] {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    // A directory with a backup's name is skipped, not deleted.
    fs::create_dir(dir.path().join("controller.sqlite.pre-v1-1.sqlite")).unwrap();
    let mut errors = Vec::new();
    let mut deleted = prune_migration_backups(&db, 2, &mut |m| errors.push(m.to_string()));
    deleted.sort();
    assert_eq!(
        deleted,
        [
            "controller.sqlite.pre-v2-100.sqlite",
            "controller.sqlite.pre-v3-999.sqlite"
        ]
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(dir
        .path()
        .join("controller.sqlite.pre-v1-1.sqlite")
        .is_dir());
    assert!(dir
        .path()
        .join("controller.sqlite.pre-v3-1.sqlite.bak")
        .exists());
    assert!(dir.path().join("other.sqlite.pre-v1-1.sqlite").exists());
    assert!(prune_migration_backups(&db, 0, &mut |_| {}).len() == 2);
}

#[test]
fn prune_reports_an_unreadable_directory_instead_of_failing() {
    let mut errors = Vec::new();
    let deleted = prune_migration_backups(
        Path::new("/nonexistent-capstan-dir/controller.sqlite"),
        3,
        &mut |m| errors.push(m.to_string()),
    );
    assert!(deleted.is_empty());
    assert!(
        errors[0].starts_with("could not prune migration backups in /nonexistent-capstan-dir: "),
        "{errors:?}"
    );
}

#[test]
fn read_only_never_creates_the_file() {
    let (dir, file) = scratch();
    assert!(open_database_read_only(&file).is_err());
    assert!(!file.exists());
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    ledger_at(&file, 2);
    let db = open_database_read_only(&file).unwrap();
    assert!(db.exec("CREATE TABLE t (x)").is_err());
}

#[test]
fn resolves_only_absolute_state_directories() {
    assert_eq!(
        resolve_database_path(Path::new("/a/b")).unwrap(),
        Path::new("/a/b/controller.sqlite")
    );
    assert_eq!(
        resolve_database_path(Path::new("a/b"))
            .unwrap_err()
            .to_string(),
        "state directory must be absolute"
    );
}

#[test]
fn applied_at_is_formatted_like_to_iso_string() {
    let fixture: serde_json::Value = serde_json::from_str(&parity_file("iso.json")).unwrap();
    let cases = fixture.as_array().unwrap();
    assert!(cases.len() > 10);
    for case in cases {
        let ms = case["ms"].as_i64().unwrap();
        assert_eq!(iso_from_millis(ms), case["iso"].as_str().unwrap(), "{ms}");
    }
}
