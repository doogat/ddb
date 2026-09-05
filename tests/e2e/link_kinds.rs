use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn all_link_kinds_indexed() {
    let repo = DdbTestRepo::init();

    // Create target doogats so wikilink/embed targets exist
    let wiki_out = repo
        .ddb()
        .args(["create", "--title", "Wiki Target", "--body", "target"])
        .output()
        .unwrap();
    assert!(
        wiki_out.status.success(),
        "fixture command failed: {wiki_out:?}"
    );
    let wiki_id = String::from_utf8_lossy(&wiki_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let embed_out = repo
        .ddb()
        .args(["create", "--title", "Embed Target", "--body", "embedded"])
        .output()
        .unwrap();
    assert!(
        embed_out.status.success(),
        "fixture command failed: {embed_out:?}"
    );
    let embed_id = String::from_utf8_lossy(&embed_out.stdout)
        .trim()
        .to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create a doogat with all 4 link types in body
    let body = format!(
        "See [[{wiki_id}]] for wiki.\n\
         Read [md title](md_target.md) for more.\n\
         Embed: ![[{embed_id}]]\n\
         Visit https://example.com for info."
    );
    let linker_out = repo
        .ddb()
        .args(["create", "--title", "Link Kinds Test", "--body", &body])
        .output()
        .unwrap();
    assert!(
        linker_out.status.success(),
        "fixture command failed: {linker_out:?}"
    );
    let linker_id = String::from_utf8_lossy(&linker_out.stdout)
        .trim()
        .to_string();

    // Reindex to populate _ddb_links
    repo.ddb().args(["reindex"]).assert().success();

    // Query all link kinds for this doogat
    repo.ddb()
        .args([
            "query",
            &format!(
                "SELECT target_path, kind FROM _ddb_links WHERE source_id = '{}' ORDER BY kind",
                linker_id
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"\burl\b").unwrap())
        .stdout(predicate::str::contains("embed"))
        .stdout(predicate::str::contains("markdown"))
        .stdout(predicate::str::contains("wikilink"));
}
