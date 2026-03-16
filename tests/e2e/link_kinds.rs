use crate::common::ZdbTestRepo;
use predicates::prelude::*;

#[test]
fn all_link_kinds_indexed() {
    let repo = ZdbTestRepo::init();

    // Create target zettels so wikilink/embed targets exist
    let wiki_out = repo
        .zdb()
        .args(["create", "--title", "Wiki Target", "--body", "target"])
        .output()
        .unwrap();
    let wiki_id = String::from_utf8_lossy(&wiki_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let embed_out = repo
        .zdb()
        .args(["create", "--title", "Embed Target", "--body", "embedded"])
        .output()
        .unwrap();
    let embed_id = String::from_utf8_lossy(&embed_out.stdout)
        .trim()
        .to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create a zettel with all 4 link types in body
    let body = format!(
        "See [[{wiki_id}]] for wiki.\n\
         Read [md title](md_target.md) for more.\n\
         Embed: ![[{embed_id}]]\n\
         Visit https://example.com for info."
    );
    let linker_out = repo
        .zdb()
        .args(["create", "--title", "Link Kinds Test", "--body", &body])
        .output()
        .unwrap();
    let linker_id = String::from_utf8_lossy(&linker_out.stdout)
        .trim()
        .to_string();

    // Reindex to populate _zdb_links
    repo.zdb().args(["reindex"]).assert().success();

    // Query all link kinds for this zettel
    repo.zdb()
        .args([
            "query",
            &format!(
                "SELECT target_path, kind FROM _zdb_links WHERE source_id = '{}' ORDER BY kind",
                linker_id
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("url"))
        .stdout(predicate::str::contains("embed"))
        .stdout(predicate::str::contains("markdown"))
        .stdout(predicate::str::contains("wikilink"));
}
