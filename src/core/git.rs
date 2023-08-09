use std::{path::Path, process::Stdio};

use chrono::{DateTime, Utc};
use color_eyre::eyre::{eyre, Context, Result};
use ignore::WalkBuilder;
use reqwest::Url;
use tempdir::TempDir;
use tracing::{debug, error, info, log::warn, trace};

use crate::{
    core::Metadata,
    store::{RepoSourceGitDirectoryPath, RepoSourceGitWorkTreePath, Store},
};

use super::GitRepo;

pub(crate) struct Git;

pub(crate) enum GitRepoStatus {
    Clean,
    Modified,
    Invalid,
}

impl Git {
    pub(crate) fn is_locally_modified(
        context_path: impl AsRef<Path>,
        path: impl AsRef<Path>,
    ) -> Result<bool> {
        let mut child = std::process::Command::new("git")
            .arg("diff")
            .arg("--quiet")
            .arg("--exit-code")
            .arg(path.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(context_path.as_ref())
            .spawn()?;
        let exit_status = child.wait()?;
        Ok(!exit_status.success())
    }
    pub(crate) fn modification_time(
        context_path: impl AsRef<Path>,
        path: impl AsRef<Path>,
    ) -> Result<Option<DateTime<Utc>>> {
        let child = std::process::Command::new("git")
            .arg("log")
            .arg("-1")
            .arg("--pretty=%ci")
            .arg(path.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(context_path.as_ref())
            .spawn()?;
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(
            DateTime::parse_from_str(stdout.trim(), "%Y-%m-%d %H:%M:%S %z")
                .ok()
                .map(|value| DateTime::from_utc(value.naive_utc(), Utc)),
        )
    }

    pub(crate) fn status(store: &Store, repo: &GitRepo) -> Result<GitRepoStatus> {
        let work_tree = store.repo_git_work_tree_path(repo);
        let output = std::process::Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(work_tree.as_ref())
            .spawn()?
            .wait_with_output()?;
        if output.status.success() {
            if output.stdout.is_empty() && output.stderr.is_empty() {
                Ok(GitRepoStatus::Clean)
            } else {
                Ok(GitRepoStatus::Modified)
            }
        } else {
            return Ok(GitRepoStatus::Invalid);
        }
    }

    pub(crate) fn current_commit(store: &Store, repo: &GitRepo) -> Result<Option<String>> {
        let work_tree = store.repo_git_work_tree_path(repo);
        let output = std::process::Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(work_tree.as_ref())
            .spawn()?
            .wait_with_output()?;
        if output.status.success() {
            let commit = String::from_utf8(output.stdout).unwrap().trim().to_string();
            Ok(Some(commit))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn clone(store: &Store, repo: &GitRepo) -> Result<()> {
        let git_directory = store.repo_git_directory_path(&repo);
        if git_directory.as_ref().is_dir() {
            return Ok(());
        }
        // Make temporary directory
        let temp_git_directory = TempDir::new_in(store.temp_dir_path().as_ref(), "git-checkout")?;
        // Initialize the git repo
        info!(
            target: "user-log",
            "Cloning git repository {}",
            repo.url,
        );
        let exit_status = std::process::Command::new("git")
            .arg("clone")
            .arg("--bare")
            .arg(repo.url.to_string())
            .arg(temp_git_directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(temp_git_directory.path())
            .spawn()?
            .wait()?;
        if !exit_status.success() {
            return Err(eyre!("Failed to clone git repository {}", repo.url,));
        }

        std::fs::create_dir_all(git_directory.as_ref().parent().unwrap()).with_context(|| {
            format!(
                "Failed to create store directory {} for git repositories",
                git_directory.as_ref().parent().unwrap().display()
            )
        })?;
        std::fs::rename(temp_git_directory.path(), git_directory.as_ref()).with_context(|| {
            format!(
                "Failed to move git repository to store directory {}",
                git_directory.as_ref().display()
            )
        })?;
        temp_git_directory.close().err().expect(
            format!(
                "Failed to clean up temporary directory for git repository {}",
                repo.url
            )
            .as_str(),
        );
        Ok(())
    }

    pub(crate) fn checkout(store: &Store, repo: &GitRepo) -> Result<RepoSourceGitWorkTreePath> {
        let git_directory = store.repo_git_directory_path(repo);
        let work_tree = store.repo_git_work_tree_path(repo);
        if work_tree.as_ref().is_dir() {
            match Git::status(store, repo)? {
                GitRepoStatus::Clean => {
                    // Update the git repo if the current commit does not match
                    if let Some(commit) = Git::current_commit(store, repo)? {
                        if commit == repo.commit {
                            return Ok(work_tree);
                        } else {
                            warn!(
                                target: "user-log",
                                "Removing modified working tree {} for git repository {}",
                                work_tree.as_ref().display(),
                                repo.url,
                            );
                            std::fs::remove_dir_all(work_tree.as_ref())?;
                        }
                    } else {
                        warn!(
                            target: "user-log",
                            "Removing empty working tree {} for git repository {}",
                            work_tree.as_ref().display(),
                            repo.url,
                        );
                        std::fs::remove_dir_all(work_tree.as_ref())?;
                    }
                }
                GitRepoStatus::Modified => {
                    warn!(
                        target: "user-log",
                        "Removing modified working tree {} for git repository {}",
                        work_tree.as_ref().display(),
                        repo.url,
                    );
                    std::fs::remove_dir_all(work_tree.as_ref())?;
                }
                GitRepoStatus::Invalid => {
                    warn!(
                        target: "user-log",
                        "Removing invalid working tree {} for git repository {}",
                        work_tree.as_ref().display(),
                        repo.url,
                    );
                    std::fs::remove_dir_all(work_tree.as_ref())?;
                }
            }
        }
        // Make temporary directory
        let temp_work_tree = TempDir::new_in(store.temp_dir_path().as_ref(), "git-work-tree")?;

        // Check if there are any changes to the repo folder
        let exit_status = std::process::Command::new("git")
            .arg("clone")
            .arg(git_directory.as_ref())
            .arg(temp_work_tree.as_ref())
            .current_dir(temp_work_tree.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
            .wait()?;
        if !exit_status.success() {
            return Err(eyre!(
                "Failed to checkout working tree for git repository {}",
                repo.url,
            ));
        }
        let exit_status = std::process::Command::new("git")
            .arg("checkout")
            .arg(&repo.commit)
            .current_dir(temp_work_tree.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
            .wait()?;
        if !exit_status.success() {
            return Err(eyre!(
                "Failed to checkout commit {} for git repository {}",
                repo.commit,
                repo.url,
            ));
        }

        std::fs::create_dir_all(work_tree.as_ref().parent().unwrap()).with_context(|| {
            format!(
                "Failed to create store directory {} for git working trees",
                git_directory.as_ref().parent().unwrap().display()
            )
        })?;
        std::fs::rename(temp_work_tree.path(), work_tree.as_ref()).with_context(|| {
            format!(
                "Failed to move git working tree to store directory {}",
                work_tree.as_ref().display()
            )
        })?;
        temp_work_tree.close().err().expect(
            format!(
                "Failed to clean up temporary directory for git repository {}",
                repo.url
            )
            .as_str(),
        );

        // Sync timestamps
        let work_tree_walker = WalkBuilder::new(work_tree.as_ref())
            .filter_entry(|p| {
                if let Some(".git") = p.path().file_name().map_or(None, |p| p.to_str()) {
                    false
                } else {
                    true
                }
            })
            .sort_by_file_path(|a, b| a.cmp(b))
            .build_parallel();
        info!(
            target: "user-log",
            "Checked out commit {} from repository {}",
            repo.commit,
            repo.url
        );
        Ok(work_tree)
    }
}
