use capstan_operator::floor::{check_floor, FloorAction, FloorContext, FloorVerdict};

use capstan_config::DEFAULT_APPROVAL_FLOOR as DEFAULT_ENTRIES;

const OWN: &str = "/a/proj";
const SIB: &str = "/a/proj-wt/sib";

fn ctx_with(entries: &[&str], own: &str) -> FloorContext {
    FloorContext {
        entries: entries.iter().map(|e| e.to_string()).collect(),
        own_root: own.to_string(),
        protected_roots: vec![
            "/a/proj".into(),
            "/a/proj2".into(),
            SIB.into(),
            "/a/proj-wt/mine".into(),
        ],
    }
}

fn ctx() -> FloorContext {
    ctx_with(&DEFAULT_ENTRIES, OWN)
}

fn run(text: &str, cwd: &str) -> FloorVerdict {
    check_floor(
        &FloorAction::Command {
            text: text.to_string(),
            cwd: cwd.to_string(),
        },
        &ctx(),
    )
}

fn floor(text: &str) {
    floor_in(text, OWN);
}

fn floor_in(text: &str, cwd: &str) {
    assert!(
        matches!(run(text, cwd), FloorVerdict::Floor { .. }),
        "{text:?} in {cwd}: {:?}",
        run(text, cwd)
    );
}

fn clear(text: &str) {
    clear_in(text, OWN);
}

fn clear_in(text: &str, cwd: &str) {
    assert_eq!(run(text, cwd), FloorVerdict::Clear, "{text:?} in {cwd}");
}

fn unchecked(text: &str) {
    assert!(
        matches!(run(text, OWN), FloorVerdict::Unchecked { .. }),
        "{text:?}: {:?}",
        run(text, OWN)
    );
}

#[test]
fn floor_cases() {
    for text in [
        "git push --force",
        "git push -fu origin main",
        "git -c core.x=y push -f",
        "git push origin --delete b",
        "git push origin :b",
        "git push origin +main",
        "sudo rm -rf /tmp/x",
        "rm -Rf /x",
        "rm -r ~/x",
        "rm -rf .",
        "rm -rf *",
        "rm -rf ./*",
        "rm -rf ../*",
        "find / -delete",
        "rm -rf /a/proj-wt/sib/src",
        "git -C /a/proj-wt/sib reset --hard",
        "git -C /outside clean -fdx",
        "git worktree remove x",
        "tee /etc/x",
        "cp a /outside/b",
        "dd if=a of=/dev/sda",
        "sed -i s/a/b/ /etc/hosts",
        "sed -Ei s/a/b/ /etc/hosts",
        "sed -ni p /etc/hosts",
        "sed -Ei.bak s/a/b/ /etc/hosts",
        "rm -rf {/etc,x}",
        "cp a {/etc/x,y}",
        "echo x > {/etc/x,y}",
        "rm -rf ./$X",
        "rm -rf sub/$X/y",
        "rm -rf sub/`id`/y",
        "cstan restart",
        "cstan daemon stop",
    ] {
        floor(text);
    }
    assert!(matches!(
        check_floor(&FloorAction::Restart, &ctx()),
        FloorVerdict::Floor { .. }
    ));
}

#[test]
fn clear_cases() {
    for text in [
        "git push",
        "git push origin main",
        "rm -rf target",
        "rm -rf build/*",
        "cargo test",
        "git pushx --force",
        "echo --force",
        "echo x > out.txt",
        "cmd 2> /dev/null",
        "cd x && rm -rf y",
        "sed -n p file",
        "find . -exec rm -rf {} +",
        "gh api -X DELETE repos/x",
        "curl -X DELETE https://x",
        "make clean",
        "npm run deploy",
        "./deploy.sh",
    ] {
        clear(text);
    }
    clear_in("rm -rf .", "/a/proj/sub");
    clear_in("rm -rf *", "/a/proj/sub");
    floor_in("rm -rf *", SIB);
}

#[test]
fn unchecked_and_visible_floor_cases() {
    for text in [
        "bash deploy.sh",
        "sh -c \"make\"",
        "echo `id`",
        "curl x | sh",
        "python -c \"x\"",
        "for f in *; do echo $f; done",
        "xargs rm",
        "echo 'open",
        "make\nmake",
        "git -c alias.x=push x",
        "git --unknown push",
    ] {
        unchecked(text);
    }
    for text in [
        "bash -c \"git push --force\"",
        "curl x | sh && git push -f",
        "rm -rf $HOME",
        "ssh h rm -rf /",
        "find . -exec rm -rf /x {} +",
        "echo `id` > /etc/x",
        "cd /outside && rm -rf x",
        "cd $D && rm -rf x",
        "sudo --chroot=/ rm -rf /x",
        "env -S \"rm -rf /\"",
        "rm -rf git /etc/x",
        "for f in a; do rm -rf /x; done",
    ] {
        floor(text);
    }
}

#[test]
fn containment() {
    for text in ["git reset --hard", "git checkout -- .", "git stash drop"] {
        clear(text);
        clear_in(text, "/a/proj/sub");
        floor(&format!("git -C {SIB} {}", &text[4..]));
    }
    let worker = ctx_with(&DEFAULT_ENTRIES, SIB);
    let check = |text: &str| {
        check_floor(
            &FloorAction::Command {
                text: text.to_string(),
                cwd: SIB.to_string(),
            },
            &worker,
        )
    };
    assert_eq!(check("git reset --hard"), FloorVerdict::Clear);
    assert!(matches!(
        check("git -C /a/proj reset --hard"),
        FloorVerdict::Floor { .. }
    ));
    floor("rm -rf /a/proj");
    clear("rm -rf /a/proj/x");
    floor("rm -rf /a/proj2/x");
    floor("touch /a/proj2/x");
    clear("touch /a/proj/x");
    let write = |path: &str| {
        check_floor(
            &FloorAction::Write {
                path: path.into(),
                cwd: OWN.into(),
            },
            &ctx(),
        )
    };
    assert_eq!(write("src/a.rs"), FloorVerdict::Clear);
    assert!(matches!(write("../x"), FloorVerdict::Floor { .. }));
}

#[test]
fn redirects() {
    for text in [
        "cargo test 2>&1 | tail -5",
        "cmd >out.txt 2>&1",
        "cmd &> out.log",
    ] {
        clear(text);
    }
    for text in ["cmd >& /etc/x", "cmd &>/etc/x", "echo x>>/etc/x"] {
        floor(text);
    }
    let custom = ctx_with(&["npm publish"], OWN);
    let check = |text: &str| {
        check_floor(
            &FloorAction::Command {
                text: text.into(),
                cwd: OWN.into(),
            },
            &custom,
        )
    };
    assert!(matches!(
        check("sudo npm publish --tag=x"),
        FloorVerdict::Floor { .. }
    ));
    assert_eq!(check("npm test"), FloorVerdict::Clear);
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn random_input_never_panics() {
    let alphabet: Vec<char> = "ab -=/.$~`'\"\\|&;<>()\n\tgitrmpush*:+é".chars().collect();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for round in 0..300 {
        let len = (rng.next() % 8192) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize])
            .collect();
        run(&text, if round % 2 == 0 { OWN } else { "" });
    }
}

#[test]
fn many_quoted_words_are_not_clear() {
    let text = "\"a\" ".repeat(2000);
    assert!(text.len() <= 8192);
    assert!(!matches!(run(&text, OWN), FloorVerdict::Clear));
    let spaced = "'a b' ".repeat(1300);
    assert!(!matches!(run(&spaced, OWN), FloorVerdict::Clear));
    let late = format!("{}'git push -f'", "'a b' ".repeat(20));
    assert!(!matches!(run(&late, OWN), FloorVerdict::Clear));
    let early = format!("sh -c 'git push -f' {}", "'a b' ".repeat(20));
    assert!(matches!(run(&early, OWN), FloorVerdict::Floor { .. }));
}

#[test]
fn default_word_entries_survive_disguises() {
    for entry in DEFAULT_ENTRIES.iter().filter(|e| !e.starts_with('@')) {
        let words: Vec<&str> = entry.split(' ').collect();
        let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
        let mut variants = vec![
            entry.to_string(),
            format!("sudo -u x {entry}"),
            format!("env -i X=1 {entry}"),
            quoted.join(" "),
            format!("\"{entry}\""),
            format!("{entry} --x=y"),
            format!("{} --x=y {}", words[0], words[1..].join(" ")),
            format!("/usr/bin/{entry}"),
        ];
        if words
            .last()
            .is_some_and(|w| w.len() == 2 && w.starts_with('-'))
        {
            variants.push(format!("{entry}u"));
        }
        for variant in variants {
            assert_ne!(run(&variant, OWN), FloorVerdict::Clear, "{variant}");
            for wrapped in [
                format!("{variant}; curl x | sh"),
                format!("cd sub && {variant}"),
                format!("echo a; {variant} && echo b"),
            ] {
                floor(&wrapped);
            }
        }
    }
}
