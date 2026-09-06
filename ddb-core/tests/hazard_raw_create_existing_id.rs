//! Hazard H4: raw FFI create reusing an id that already exists.
//!
//! `DoogatService::create_doogat_raw` (ddb-core/src/service/create.rs:431-434)
//! takes the `id:` from the caller's frontmatter verbatim when present and only
//! consults the repo-aware minting oracle (write_helpers.rs:244-247,
//! id_minting.rs:302) when the frontmatter has NO id. It then calls
//! `repo.commit_file(&rel_path, content, ..)` (create.rs:488), and
//! `commit_files` (git_ops/mod.rs:366-371) is a plain `std::fs::write` +
//! re-stage of `ddb/{id}.md`. Nothing on that path checks whether `ddb/{id}.md`
//! already exists, so an FFI caller (`DoogatDriver::create_doogat`,
//! ffi/driver.rs:66-71) that ships frontmatter naming an existing id silently
//! REPLACES that doogat's file in git and re-indexes it under the new content.
//!
//! Safe behavior pinned here: the raw create either rejects the collision or
//! re-mints a fresh id; in every case doogat A's stored bytes stay unchanged.
//! A failure of this test means create-as-overwrite data loss through FFI.

use ddb_core::service::DoogatService;

#[test]
fn raw_create_with_existing_id_rejects_or_remints_and_never_overwrites() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    // Doogat A via the normal (minting) service path.
    let a_id = svc
        .create_doogat("Original A", &[], None, "Original body of A.")
        .expect("normal create of A must succeed");
    let a_before = svc
        .read_doogat(&a_id)
        .expect("A must be readable right after creation");
    assert!(
        a_before.contains("title: Original A") && a_before.contains("Original body of A."),
        "precondition: A's stored content must carry its title and body; got:\n{a_before}"
    );

    // Raw FFI create whose frontmatter names A's id with different title/body.
    let impostor = format!(
        "\
---
id: {a_id}
title: Impostor
---
Impostor body that must never replace A.
"
    );
    let outcome = svc.create_doogat_raw(&impostor, "raw create reusing A's id");

    // Regardless of Ok/Err, A's bytes must be exactly what they were.
    let a_after = svc
        .read_doogat(&a_id)
        .expect("H4 fired: doogat A is no longer readable after a raw create reused its id");
    assert_eq!(
        a_after, a_before,
        "H4 fired: create_doogat_raw with an existing id OVERWROTE doogat A \
         (id {a_id}) instead of rejecting or re-minting.\nbefore:\n{a_before}\nafter:\n{a_after}"
    );

    match outcome {
        // Rejecting the collision is one of the two safe outcomes.
        Err(_) => {}
        // Re-minting is the other: a NEW id must hold the impostor content.
        Ok(new_id) => {
            assert_ne!(
                new_id, a_id,
                "H4 fired: create_doogat_raw returned the pre-existing id {a_id} \
                 as if it had created a new doogat"
            );
            let stored_new = svc
                .read_doogat(&new_id)
                .expect("re-minted raw doogat must be readable under its new id");
            assert!(
                stored_new.contains("title: Impostor")
                    && stored_new.contains("Impostor body that must never replace A."),
                "re-minted raw doogat {new_id} must carry the raw content; got:\n{stored_new}"
            );
        }
    }
}
