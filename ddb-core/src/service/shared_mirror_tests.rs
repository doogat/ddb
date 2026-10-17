//! Two services on one repo (the server's write actor and maintenance worker)
//! share ONE NoSQL mirror handle. Mirror writes from both must land: redb
//! refuses a second concurrent open of the same file in one process, so the
//! shared handle has to serialize them.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::DoogatService;
use crate::app_contract::{CreateCommand, UnregisteredTypePolicy};
use crate::git_ops::GitRepo;
use crate::indexer::Index;

fn create_cmd(title: String) -> CreateCommand {
    CreateCommand {
        title: Some(title),
        body: None,
        tags: vec![],
        doogat_type: None,
        fields: BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
    }
}

#[test]
fn services_sharing_one_mirror_both_land_mirror_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();
    drop(DoogatService::init(&path).unwrap());
    let mirror = DoogatService::shared_mirror(&path);

    let workers: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| {
            let path = path.clone();
            let mirror = Arc::clone(&mirror);
            std::thread::spawn(move || {
                let repo = GitRepo::open(&path).unwrap();
                let index = Index::open(&path.join(".ddb/index.db")).unwrap();
                let svc = DoogatService::from_parts(repo, index, Box::new(mirror), path);
                (0..3)
                    .map(|i| {
                        let out = svc.create(create_cmd(format!("{name}-{i}"))).unwrap();
                        out.value.meta.id.unwrap().0
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let ids: Vec<String> = workers
        .into_iter()
        .flat_map(|w| w.join().unwrap())
        .collect();

    let redb = crate::nosql::RedbIndex::open(&path.join(".ddb/nosql.redb")).unwrap();
    for id in &ids {
        assert!(
            redb.get(id).unwrap().is_some(),
            "mirror write for {id} was dropped"
        );
    }
}
