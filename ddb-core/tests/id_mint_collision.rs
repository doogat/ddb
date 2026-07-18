use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;

#[test]
fn raw_create_and_batch_mint_distinct_ids_in_one_second() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();

    // Seed the current second's ID so both mint paths must skip it.
    let seed_id = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    let seeded_content = format!(
        "\
---
id: {seed_id}
title: SeedDoogat
---
Seed body content.
"
    );
    let returned_seed_id = svc
        .create_doogat_raw(&seeded_content, "seed current-second id")
        .expect("create_doogat_raw must accept an author-supplied id");
    assert_eq!(returned_seed_id, seed_id, "author-supplied id must be stored verbatim");

    // Mint via raw-create path (no explicit id).
    let raw_content = "\
---
title: RawMintedDoogat
---
Raw minted body.
";
    let raw_id = svc
        .create_doogat_raw(raw_content, "mint via raw create")
        .expect("create_doogat_raw must mint an id when none is supplied");

    // Mint via SQL batch INSERT path.
    svc.execute_sql("CREATE TABLE sm1item (label TEXT)")
        .expect("CREATE TABLE must succeed");
    let insert_result = svc
        .execute_sql("INSERT INTO sm1item (title, label) VALUES ('BatchRow', 'b1')")
        .expect("INSERT must succeed");
    let batch_id = match insert_result {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id) from batch INSERT, got {other:?}"),
    };

    // All three IDs must be distinct.
    assert_ne!(
        raw_id, seed_id,
        "raw-minted id must not collide with the pre-existing current-second id"
    );
    assert_ne!(
        batch_id, seed_id,
        "batch-minted id must not collide with the pre-existing current-second id"
    );
    assert_ne!(
        raw_id, batch_id,
        "raw-create and batch INSERT must not collide with each other"
    );

    // Verify no pre-existing content was overwritten.
    let stored_seed = svc
        .read_doogat(&seed_id)
        .expect("read_doogat must still find the seeded doogat");
    assert!(
        stored_seed.contains("title: SeedDoogat"),
        "seeded doogat must not be overwritten; got:\n{stored_seed}"
    );

    // Verify both newly minted doogats are readable.
    let stored_raw = svc
        .read_doogat(&raw_id)
        .expect("read_doogat must find the raw-minted doogat");
    assert!(
        stored_raw.contains("title: RawMintedDoogat"),
        "raw-minted doogat content must survive; got:\n{stored_raw}"
    );
}

#[test]
fn batch_then_raw_create_never_reuses_a_batch_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();

    // Batch INSERT N rows via one multi-row INSERT (a single git commit), so
    // all ids are minted in one shot into a contiguous reserved window
    // [base, base+N-1].
    //
    // N is deliberately 5, not the 2-3 the design first suggested. The old bug
    // only reproduces in-process when the later `create_doogat_raw` mints a
    // `now()` id that lands INSIDE the batch's reserved window. `generate_id`
    // (raw create) and the batch minter share one process-global monotonic
    // `LAST` mutex with a spin-wait, so a narrow window (N=2) is overshot by
    // commit latency and the regression silently stops failing against the old
    // code. A wider window keeps the old-code collision reliable across machine
    // speeds. Under the fixed code the single create observes every reserved id
    // and advances, so the test still blocks ~N seconds and passes.
    const BATCH_N: usize = 5;
    svc.execute_sql("CREATE TABLE sm2item (label TEXT)")
        .expect("CREATE TABLE must succeed");

    let insert_result = svc
        .execute_sql(
            "INSERT INTO sm2item (title, label) VALUES \
             ('BatchRow0', 'l0'), ('BatchRow1', 'l1'), ('BatchRow2', 'l2'), \
             ('BatchRow3', 'l3'), ('BatchRow4', 'l4')",
        )
        .expect("multi-row INSERT must succeed");
    let batch_ids: Vec<String> = match insert_result {
        SqlResult::Ok(ids) => ids.split(',').map(|s| s.to_string()).collect(),
        other => panic!("expected Ok(ids) from multi-row INSERT, got {other:?}"),
    };
    assert_eq!(
        batch_ids.len(),
        BATCH_N,
        "multi-row INSERT must mint {BATCH_N} ids, got {batch_ids:?}"
    );

    // Batch IDs must be distinct from each other.
    let unique: std::collections::HashSet<&String> = batch_ids.iter().collect();
    assert_eq!(
        unique.len(),
        batch_ids.len(),
        "batch IDs must all be distinct"
    );

    // Now mint ONE id via create_doogat_raw (no frontmatter id).
    // NOTE: Must use create_doogat_raw, NOT normal create — normal create
    // was already repo-aware, so this test would pass against the OLD buggy
    // code and is not a valid regression. create_doogat_raw used the
    // buggy `|_| false` existence check.
    let raw_content = "\
---
title: AfterBatchDoogat
---
Body after batch insert.
";
    // The minter waits on the wall clock until the batch's reserved window
    // passes (~BATCH_N seconds), so this blocks ~5s.
    let single_id = svc
        .create_doogat_raw(raw_content, "mint after batch")
        .expect("create_doogat_raw must mint an id after batch");

    // The single minted id must not reuse any batch id.
    for (i, batch_id) in batch_ids.iter().enumerate() {
        assert_ne!(
            single_id, *batch_id,
            "single raw-create id must not collide with batch id {i} ({batch_id})"
        );
    }

    // Every batch row's content must still survive.
    for (i, batch_id) in batch_ids.iter().enumerate() {
        let stored = svc
            .read_doogat(batch_id)
            .expect("read_doogat must find batch row {i}");
        assert!(
            stored.contains(&format!("title: BatchRow{i}")),
            "batch row {i} content must survive; got:\n{stored}"
        );
    }

    // The after-batch doogat must also be readable.
    let stored_single = svc
        .read_doogat(&single_id)
        .expect("read_doogat must find the after-batch doogat");
    assert!(
        stored_single.contains("title: AfterBatchDoogat"),
        "after-batch doogat content must survive; got:\n{stored_single}"
    );
}
