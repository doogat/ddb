use std::time::{Duration, Instant};

use super::GitRepo;
use crate::error::{DoogatError, Result};

/// Default bound for one remote fetch or push.
pub const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

/// Slack for libgit2 to unwind after its own timeout fires, so a call that
/// hit the idle timeout is still classified as `REMOTE_TIMEOUT`.
const UNWIND_MARGIN: Duration = Duration::from_millis(200);

/// Install libgit2's process-wide connect and socket-idle timeouts. The idle
/// timeout aborts any fetch or push that sees no bytes in either direction
/// for `timeout`. Errors if the linked libgit2 does not support the options.
pub fn set_libgit2_remote_timeouts(timeout: Duration) -> Result<()> {
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: these calls only set libgit2 global integers; git2 initialises
    // libgit2 before setting them, and no pointers are passed.
    unsafe {
        git2::opts::set_server_connect_timeout_in_milliseconds(millis)?;
        git2::opts::set_server_timeout_in_milliseconds(millis)?;
    }
    Ok(())
}

/// Fetch callbacks that abort the transfer once `deadline` passes. Push has
/// no equivalent: git2's push progress callbacks cannot abort.
fn remote_callbacks(deadline: Instant) -> git2::RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.transfer_progress(move |_| Instant::now() < deadline);
    callbacks.sideband_progress(move |_| Instant::now() < deadline);
    callbacks
}

/// Map a failed remote call: past the deadline it is `REMOTE_TIMEOUT`,
/// whatever shape the git2 error has; otherwise the original git error.
fn map_remote_error(
    err: git2::Error,
    operation: &str,
    start: Instant,
    timeout: Duration,
) -> DoogatError {
    if start.elapsed() + UNWIND_MARGIN >= timeout {
        DoogatError::remote_timeout(operation, timeout)
    } else {
        err.into()
    }
}

impl GitRepo {
    /// Add a named remote.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote(name, url)?;
        Ok(())
    }

    /// Fetch from a remote, bounded by `timeout`.
    pub fn fetch(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        let start = Instant::now();
        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(remote_callbacks(start + timeout));
        remote
            .fetch(&[branch], Some(&mut opts), None)
            .map_err(|e| map_remote_error(e, "fetch", start, timeout))
    }

    /// Push to a remote, bounded by `timeout`.
    pub fn push(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        let start = Instant::now();
        remote
            .push(&[&refspec], None)
            .map_err(|e| map_remote_error(e, "push", start, timeout))
    }

    /// Delete a remote-tracking ref (`refs/remotes/{remote}/{branch}`), if it exists.
    pub fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()> {
        self.with_write_lock(|| {
            let ref_name = format!("refs/remotes/{remote}/{branch}");
            match self.repo.find_reference(&ref_name) {
                Ok(mut reference) => {
                    reference.delete()?;
                    Ok(())
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
    }
}
