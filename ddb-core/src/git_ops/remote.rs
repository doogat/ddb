use super::GitRepo;
use crate::error::Result;

impl GitRepo {
    /// Add a named remote.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote(name, url)?;
        Ok(())
    }

    /// Fetch from a remote.
    pub fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        remote.fetch(&[branch], None, None)?;
        Ok(())
    }

    /// Push to a remote.
    pub fn push(&self, remote: &str, branch: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        remote.push(&[&refspec], None)?;
        Ok(())
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
