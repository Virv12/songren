use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tracing::info;

use crate::config::{GitConfig, GlobalConfig};

pub fn repo_path(cfg: &GlobalConfig, service: &str, name: &str) -> PathBuf {
    cfg.repos_dir.join(service).join(format!("{name}.git"))
}

pub fn worktree_path(cfg: &GlobalConfig, service: &str, name: &str, git: &GitConfig) -> PathBuf {
    git.path
        .clone()
        .unwrap_or_else(|| cfg.worktrees_dir.join(service).join(name))
}

async fn run_git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args[0], stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        info!(path = %path.display(), "Removing directory");
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
    }
    Ok(())
}

async fn sync_single_repo(repo_dir: &Path, work_tree: &Path, git: &GitConfig) -> Result<()> {
    let repo_str = repo_dir
        .to_str()
        .with_context(|| format!("Non-UTF8 repo path: {}", repo_dir.display()))?;
    let tree_str = work_tree
        .to_str()
        .with_context(|| format!("Non-UTF8 worktree path: {}", work_tree.display()))?;

    if !repo_dir.join("HEAD").exists() {
        if let Some(parent) = repo_dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent dir: {}", parent.display()))?;
        }
        info!(repo = %git.repo, repo_dir = %repo_dir.display(), "Initializing bare git repository");
        run_git(&["init", "--bare", repo_str]).await?;
    }

    std::fs::create_dir_all(work_tree)
        .with_context(|| format!("Failed to create worktree dir: {}", work_tree.display()))?;

    let target = git.branch.as_deref().unwrap_or("HEAD");
    info!(
        repo = %git.repo,
        work_tree = %work_tree.display(),
        "Fetching and checking out static files"
    );
    run_git(&[
        "--git-dir",
        repo_str,
        "fetch",
        "--depth",
        "1",
        &git.repo,
        target,
    ])
    .await?;
    run_git(&[
        "--git-dir",
        repo_str,
        "--work-tree",
        tree_str,
        "checkout",
        "-f",
        "FETCH_HEAD",
    ])
    .await?;
    run_git(&[
        "--git-dir",
        repo_str,
        "--work-tree",
        tree_str,
        "clean",
        "-fd",
    ])
    .await?;

    Ok(())
}

pub async fn sync_git(
    cfg: &GlobalConfig,
    service: &str,
    desired: &BTreeMap<String, GitConfig>,
    old: &BTreeMap<String, GitConfig>,
) -> Result<BTreeMap<String, PathBuf>> {
    for (name, old_git) in old {
        let old_tree = worktree_path(cfg, service, name, old_git);
        let old_repo = repo_path(cfg, service, name);
        match desired.get(name) {
            None => {
                remove_dir_if_exists(&old_tree)?;
                remove_dir_if_exists(&old_repo)?;
            }
            Some(new_git) => {
                let new_tree = worktree_path(cfg, service, name, new_git);
                if old_tree != new_tree {
                    remove_dir_if_exists(&old_tree)?;
                }
            }
        }
    }

    let mut sites = BTreeMap::new();
    for (name, git) in desired {
        let repo_dir = repo_path(cfg, service, name);
        let work_tree = worktree_path(cfg, service, name, git);
        sync_single_repo(&repo_dir, &work_tree, git).await?;
        sites.insert(name.clone(), work_tree);
    }

    Ok(sites)
}

pub fn remove_all_git(
    cfg: &GlobalConfig,
    service: &str,
    old: &BTreeMap<String, GitConfig>,
) -> Result<()> {
    for (name, git) in old {
        remove_dir_if_exists(&worktree_path(cfg, service, name, git))?;
        remove_dir_if_exists(&repo_path(cfg, service, name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn resolves_repo_and_worktree_paths() {
        let tmp = tempdir().unwrap();
        let mut global: GlobalConfig = toml::from_str(r#"listen_on = "127.0.0.1:8000""#).unwrap();
        global.resolve_dirs(tmp.path());

        let default_git = GitConfig {
            repo: "https://example.com/site.git".to_owned(),
            branch: None,
            path: None,
        };
        assert_eq!(
            repo_path(&global, "docs", "main"),
            tmp.path().join("repos").join("docs").join("main.git")
        );
        assert_eq!(
            worktree_path(&global, "docs", "main", &default_git),
            tmp.path().join("worktrees").join("docs").join("main")
        );

        let custom_git = GitConfig {
            repo: "https://example.com/site.git".to_owned(),
            branch: Some("prod".to_owned()),
            path: Some(PathBuf::from("/var/www/custom")),
        };
        assert_eq!(
            worktree_path(&global, "docs", "main", &custom_git),
            PathBuf::from("/var/www/custom")
        );
    }
}
