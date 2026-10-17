//! Stalled-remote fixtures for the bounded fetch/push path.
//!
//! Each fixture is a plain TCP listener speaking just enough smart-HTTP to
//! put libgit2 in a known state, then going silent: either before any byte
//! (stall before progress) or after a partial ref advertisement (stall
//! mid-transfer). libgit2's process-wide idle timeout must abort the call,
//! and `fetch`/`push` must surface it as `REMOTE_TIMEOUT`.

use super::{set_libgit2_remote_timeouts, GitRepo};
use crate::error::{codes, DoogatError};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const BOUND: Duration = Duration::from_secs(1);

const FETCH_PARTIAL: &[u8] = b"HTTP/1.1 200 OK\r\n\
Content-Type: application/x-git-upload-pack-advertisement\r\n\
Content-Length: 4096\r\n\r\n\
001e# service=git-upload-pack\n0000";

const PUSH_PARTIAL: &[u8] = b"HTTP/1.1 200 OK\r\n\
Content-Type: application/x-git-receive-pack-advertisement\r\n\
Content-Length: 4096\r\n\r\n\
001f# service=git-receive-pack\n0000";

#[derive(Clone, Copy)]
enum Op {
    Fetch,
    Push,
}

/// Start a server that accepts one connection, reads the request, writes
/// `prefix`, then stalls. The receiver fires once the client has closed the
/// socket, proving no transfer is left running after the call returns.
fn stalled_remote(prefix: &'static [u8]) -> (String, mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    let (closed_tx, closed_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(prefix);
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = closed_tx.send(());
    });
    (url, closed_rx)
}

fn run(repo: &GitRepo, op: Op) -> crate::error::Result<()> {
    match op {
        Op::Fetch => repo.fetch("origin", "master", BOUND),
        Op::Push => repo.push("origin", "master", BOUND),
    }
}

fn assert_remote_timeout(err: &DoogatError) {
    assert!(
        matches!(err, DoogatError::Structured { code, .. } if *code == codes::REMOTE_TIMEOUT),
        "expected REMOTE_TIMEOUT, got {err:?}"
    );
}

fn assert_stall_times_out(op: Op, prefix: &'static [u8]) {
    set_libgit2_remote_timeouts(BOUND).unwrap();
    let (url, closed) = stalled_remote(prefix);
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.add_remote("origin", &url).unwrap();
    let head_before = repo.head_oid().unwrap();

    let start = Instant::now();
    let err = run(&repo, op).unwrap_err();
    let elapsed = start.elapsed();

    assert_remote_timeout(&err);
    assert!(elapsed < BOUND * 4, "returned after {elapsed:?}, bound {BOUND:?}");
    closed
        .recv_timeout(Duration::from_secs(2))
        .expect("connection still open after the call returned");
    assert_eq!(repo.head_oid().unwrap(), head_before, "HEAD moved");
    assert!(
        repo.repo.find_reference("refs/remotes/origin/master").is_err(),
        "a remote-tracking ref was written by a timed-out fetch"
    );
    assert_eq!(repo.repo.state(), git2::RepositoryState::Clean);
}

#[test]
fn fetch_stalled_before_progress_returns_remote_timeout() {
    assert_stall_times_out(Op::Fetch, b"");
}

#[test]
fn fetch_stalled_mid_transfer_returns_remote_timeout() {
    assert_stall_times_out(Op::Fetch, FETCH_PARTIAL);
}

#[test]
fn push_stalled_before_progress_returns_remote_timeout() {
    assert_stall_times_out(Op::Push, b"");
}

#[test]
fn push_stalled_mid_transfer_returns_remote_timeout() {
    assert_stall_times_out(Op::Push, PUSH_PARTIAL);
}

/// Network I/O must not hold the repo write lock: a local commit lands while
/// a fetch on the same repository is blocked on a stalled remote.
#[test]
fn unrelated_writer_proceeds_during_blocked_fetch() {
    set_libgit2_remote_timeouts(BOUND).unwrap();
    let (url, _closed) = stalled_remote(b"");
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.add_remote("origin", &url).unwrap();

    let path = dir.path().to_path_buf();
    let (done_tx, done_rx) = mpsc::channel();
    let fetcher = std::thread::spawn(move || {
        let repo = GitRepo::open(&path).unwrap();
        let result = repo.fetch("origin", "master", BOUND);
        let _ = done_tx.send(());
        result
    });

    std::thread::sleep(Duration::from_millis(200));
    repo.commit_file("ddb/local.md", "local", "local write")
        .unwrap();
    assert!(
        done_rx.try_recv().is_err(),
        "fetch finished before the writer ran; the fixture did not stall"
    );

    let err = fetcher.join().unwrap().unwrap_err();
    assert_remote_timeout(&err);
    assert_eq!(repo.read_file("ddb/local.md").unwrap(), "local");
}

/// A fast failure (nothing listening) keeps its original git error rather
/// than being misreported as a timeout.
#[test]
fn fast_failure_is_not_reported_as_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    drop(listener);
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.add_remote("origin", &url).unwrap();
    let err = repo
        .fetch("origin", "master", Duration::from_secs(30))
        .unwrap_err();
    assert!(matches!(err, DoogatError::Git(_)), "got {err:?}");
}
