use ddb_core::service::DoogatService;

#[test]
fn raw_create_mints_distinct_ids_around_a_taken_current_second_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    let seed_id = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();

    let seeded_content = format!(
        "\
---
id: {seed_id}
title: SeedDoogatOriginalTitle
---
Seed body content.
"
    );
    let returned_seed_id = svc
        .create_doogat_raw(&seeded_content, "seed current-second id")
        .expect("create_doogat_raw must accept an author-supplied id");
    assert_eq!(
        returned_seed_id, seed_id,
        "author-supplied id must be stored verbatim"
    );

    let content_no_id = "\
---
title: MintedRowNoId
---
Body content with no explicit id.
";

    let id1 = svc
        .create_doogat_raw(content_no_id, "mint first")
        .expect("create_doogat_raw must mint an id when none is supplied");
    let id2 = svc
        .create_doogat_raw(content_no_id, "mint second")
        .expect("create_doogat_raw must mint an id when none is supplied");

    assert_ne!(
        id1, seed_id,
        "first minted id must not collide with the pre-existing current-second id"
    );
    assert_ne!(
        id2, seed_id,
        "second minted id must not collide with the pre-existing current-second id"
    );
    assert_ne!(
        id1, id2,
        "two back-to-back mints must not collide with each other"
    );

    let stored_seed = svc
        .read_doogat(&seed_id)
        .expect("read_doogat must still find the seeded doogat");
    assert!(
        stored_seed.contains("title: SeedDoogatOriginalTitle"),
        "seeded doogat must not be overwritten by a later mint; got:\n{stored_seed}"
    );
}
