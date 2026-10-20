//! `release.yml` can only make a draft (DAN-1823). The file is read as text: the guarantees are
//! about what is *not* in it, so a missing guard shows up as a failing line here.

use std::path::Path;

fn workflow() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".github")
        .join("workflows")
        .join("release.yml");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

/// The lines that are code, not comments.
fn code(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end().to_owned())
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect()
}

#[test]
fn it_runs_only_when_dispatched_by_hand() {
    let lines = code(&workflow());
    let on = lines
        .iter()
        .position(|l| l == "on:")
        .expect("an `on:` block");
    let block: Vec<&str> = lines[on + 1..]
        .iter()
        .take_while(|l| l.starts_with(' ') || l.is_empty())
        .map(String::as_str)
        .collect();
    let triggers: Vec<&str> = block
        .iter()
        .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
        .map(|l| l.trim().trim_end_matches(':'))
        .collect();
    assert_eq!(triggers, ["workflow_dispatch"], "{block:?}");
}

#[test]
fn the_only_permission_is_contents_write() {
    let lines = code(&workflow());
    let at: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("permissions:"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "one permissions block, at the top");
    assert_eq!(lines[at[0]], "permissions:");
    assert_eq!(lines[at[0] + 1], "  contents: write");
    assert!(
        !lines[at[0] + 2].starts_with("  "),
        "another permission: {}",
        lines[at[0] + 2]
    );
}

#[test]
fn no_secret_but_the_job_token_and_no_untrusted_text_in_a_script() {
    for l in code(&workflow()) {
        assert!(!l.contains("secrets."), "a secret: {l}");
        if l.contains("${{") {
            // An expression is only ever the value of an environment variable (`NAME: ${{ ... }}`),
            // so the untrusted tag never lands inside a script body.
            let ok = l.trim_start().split_once(": ").is_some_and(|(k, v)| {
                k.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    && v.starts_with("${{")
                    && v.ends_with("}}")
            });
            assert!(ok, "an expression outside `env:`: {l}");
        }
        if l.contains("github.token") {
            assert!(l.trim_start().starts_with("GH_TOKEN:"), "{l}");
        }
    }
}

#[test]
fn nothing_in_it_can_tag_publish_or_mark_latest() {
    let text = code(&workflow()).join("\n");
    for banned in [
        "git tag",
        "git push",
        "gh release edit",
        "gh release upload",
        "--draft=false",
        "--latest=true",
        "--verify-tag",
        "gh api -X POST",
        "gh api --method POST",
    ] {
        assert!(!text.contains(banned), "found `{banned}`");
    }
    // Every `--latest` is `--latest=false`.
    for part in text.split("--latest").skip(1) {
        assert!(
            part.starts_with("=false"),
            "--latest{}",
            &part[..part.len().min(12)]
        );
    }
}

#[test]
fn the_release_it_creates_is_a_draft_and_there_is_only_one_such_step() {
    let text = code(&workflow()).join("\n");
    assert_eq!(text.matches("gh release create").count(), 1);
    assert!(text.contains("flags=\"--draft --latest=false\""));
    // The draft flag is set before the command that uses `$flags`, which comes right after it.
    let create = text.find("gh release create").unwrap();
    let flags = text.find("flags=\"--draft").unwrap();
    assert!(flags < create);
    assert!(text[create..].contains("$flags"));
}

#[test]
fn the_tag_and_the_version_must_agree() {
    let text = workflow();
    assert!(text.contains(r#"[ "v$VERSION" != "$TAG" ]"#));
    assert!(text.contains("is not v<MAJOR>.<MINOR>.<PATCH>"));
}
