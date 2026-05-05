use std::path::PathBuf;

use serde::Deserialize;

/// A JSON manifest describing Bazel runfiles dependencies to populate into a virtual environment.
#[derive(Debug, Deserialize)]
#[expect(dead_code)]
pub(crate) struct BazelPthManifest {
    pub repository: String,
    pub python_version: String,
    pub entries: Vec<BazelSiteEntry>,
}

/// A single entry in the Bazel runfiles manifest.
#[derive(Debug, Deserialize)]
pub(crate) struct BazelSiteEntry {
    pub repo: String,
    pub path: PathBuf,
    pub strategy: BazelPopulationStrategy,
}

/// Strategy for populating a Bazel site entry into the virtual environment.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BazelPopulationStrategy {
    Pth,
    Symlink,
    Copy,
}
