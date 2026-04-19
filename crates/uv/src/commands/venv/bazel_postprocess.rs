use std::io;
use std::path::{Path, PathBuf};

use uv_python::PythonEnvironment;
use walkdir::WalkDir;

use crate::commands::venv::bazel_manifest::{BazelPthManifest, BazelPopulationStrategy};
use crate::commands::venv::shim_bytes::select_shim;

/// Post-process a standard virtual environment for Bazel runfiles execution.
pub(crate) fn bazel_runfiles_postprocess(
    venv_root: &Path,
    env: &PythonEnvironment,
    manifest_path: &Path,
) -> io::Result<()> {
    let manifest = fs_err::read_to_string(manifest_path)?;
    let manifest: BazelPthManifest = serde_json::from_str(&manifest)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let interpreter_rloc = env.interpreter().sys_executable().to_string_lossy();
    let repo = std::env::var("BAZEL_WORKSPACE").unwrap_or_else(|_| "_main".to_string());

    env.set_pyvenv_cfg("aspect-runfiles-interpreter", &interpreter_rloc)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    env.set_pyvenv_cfg("aspect-runfiles-repo", &repo)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    env.set_pyvenv_cfg(
        "aspect-absolute-interpreter",
        &env.interpreter().sys_executable().to_string_lossy(),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let site_packages = env
        .site_packages()
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| env.interpreter().purelib().to_path_buf());

    let runfiles_root = resolve_runfiles_root(venv_root);

    let mut pth_lines = Vec::new();
    for entry in &manifest.entries {
        let entry_source = runfiles_root
            .as_ref()
            .map(|root| root.join(&entry.repo).join(&entry.path));

        match entry.strategy {
            BazelPopulationStrategy::Pth => {
                let line = format!("../../{}/{}", entry.repo, entry.path.display());
                pth_lines.push(line);
            }
            BazelPopulationStrategy::Symlink => {
                if let Some(ref source) = entry_source {
                    populate_symlinks(&site_packages, source)?;
                } else {
                    // Create a dangling symlink as a best-effort fallback.
                    let dangling = PathBuf::from(format!(
                        "../../{}/{}",
                        entry.repo,
                        entry.path.display()
                    ));
                    let link_name = entry
                        .path
                        .file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new(&entry.repo));
                    let target = site_packages.join(link_name);
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&dangling, target)?;
                    }
                    #[cfg(windows)]
                    {
                        uv_fs::replace_symlink(&dangling, target)?;
                    }
                }
            }
            BazelPopulationStrategy::Copy => {
                if let Some(ref source) = entry_source {
                    populate_copies(&site_packages, source)?;
                }
            }
        }
    }

    if !pth_lines.is_empty() {
        let pth_file = site_packages.join("_bazel.pth");
        let contents = pth_lines.join("\n") + "\n";
        fs_err::write(pth_file, contents)?;
    }

    let interpreter = env.interpreter();
    let target = interpreter_target_triple(interpreter);
    let shim_bytes = target
        .as_deref()
        .and_then(select_shim)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "No Bazel shim available for target {}. \
                     Run `cargo build -p uv-bazel-shim --release --target <triple>` \
                     and copy the binary to crates/uv/src/commands/venv/shims/",
                    target.as_deref().unwrap_or("unknown")
                ),
            )
        })?;

    // Replace the venv python executable with the Bazel shim.
    #[cfg(unix)]
    {
        let bin_python = env.scripts().join("python");
        if bin_python.exists() || bin_python.is_symlink() {
            fs_err::remove_file(&bin_python)?;
        }
        install_bazel_shim(&bin_python, shim_bytes)?;
    }

    #[cfg(windows)]
    {
        let python_exe = env.scripts().join("python.exe");
        if python_exe.exists() {
            fs_err::remove_file(&python_exe)?;
        }
        install_bazel_shim(&python_exe, shim_bytes)?;

        let pythonw_exe = env.scripts().join("pythonw.exe");
        if pythonw_exe.exists() {
            fs_err::remove_file(&pythonw_exe)?;
        }
        install_bazel_shim(&pythonw_exe, shim_bytes)?;
    }

    Ok(())
}

/// Write the embedded shim binary to `dest` and make it executable.
fn install_bazel_shim(dest: &Path, shim_bytes: &[u8]) -> io::Result<()> {
    fs_err::write(dest, shim_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(dest, perms)?;
    }
    Ok(())
}

/// Map an interpreter platform to a Rust target triple for shim selection.
fn interpreter_target_triple(interpreter: &uv_python::Interpreter) -> Option<String> {
    let os = interpreter.os().to_string();
    let arch = interpreter.arch().to_string();
    let libc = interpreter.libc().to_string();

    match (os.as_str(), arch.as_str(), libc.as_str()) {
        ("macos", "aarch64", "none") => Some("aarch64-apple-darwin".to_string()),
        ("macos", "x86_64", "none") => Some("x86_64-apple-darwin".to_string()),
        ("linux", "aarch64", "gnu") => Some("aarch64-unknown-linux-gnu".to_string()),
        ("linux", "aarch64", "musl") => Some("aarch64-unknown-linux-musl".to_string()),
        ("linux", "x86_64", "gnu") => Some("x86_64-unknown-linux-gnu".to_string()),
        ("linux", "x86_64", "musl") => Some("x86_64-unknown-linux-musl".to_string()),
        ("windows", "x86_64", "gnu") => Some("x86_64-pc-windows-gnu".to_string()),
        ("windows", "x86_64", _) => Some("x86_64-pc-windows-msvc".to_string()),
        _ => None,
    }
}

/// Resolve the Bazel runfiles root from the environment or a sibling directory.
fn resolve_runfiles_root(venv_root: &Path) -> Option<PathBuf> {
    if let Ok(runfiles_dir) = std::env::var("RUNFILES_DIR") {
        return Some(PathBuf::from(runfiles_dir));
    }
    if let Some(parent) = venv_root.parent() {
        let sibling = parent.join(format!(
            "{}.runfiles",
            venv_root.file_name().unwrap_or_default().to_string_lossy()
        ));
        if sibling.is_dir() {
            return Some(sibling);
        }
    }
    None
}

/// Populate `site_packages` with relative symlinks pointing into `source`.
/// When possible, top-level directories are coalesced into a single directory symlink.
fn populate_symlinks(site_packages: &Path, source: &Path) -> io::Result<()> {
    if !source.exists() {
        if let Some(name) = source.file_name() {
            let target = site_packages.join(name);
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(source, target)?;
            }
            #[cfg(windows)]
            {
                uv_fs::replace_symlink(source, target)?;
            }
        }
        return Ok(());
    }

    for child in fs_err::read_dir(source)? {
        let child = child?;
        let child_source = child.path();
        let Some(child_name) = child_source.file_name() else {
            continue;
        };
        let child_target = site_packages.join(child_name);

        if child_source.is_dir() {
            if can_coalesce_directory(&child_source) {
                let link_target = uv_fs::relative_to(&child_source, site_packages)?;
                uv_fs::replace_symlink(&link_target, &child_target)?;
                continue;
            }
            for entry in WalkDir::new(&child_source) {
                let entry = entry?;
                let source_path = entry.path();
                let Ok(relative) = source_path.strip_prefix(&child_source) else {
                    continue;
                };
                let target_path = child_target.join(relative);
                if entry.file_type().is_dir() {
                    fs_err::create_dir_all(&target_path)?;
                } else {
                    if let Some(parent) = target_path.parent() {
                        fs_err::create_dir_all(parent)?;
                    }
                    let link_target = uv_fs::relative_to(source_path, target_path.parent().unwrap())?;
                    uv_fs::replace_symlink(&link_target, target_path)?;
                }
            }
        } else {
            let link_target = uv_fs::relative_to(&child_source, site_packages)?;
            uv_fs::replace_symlink(&link_target, &child_target)?;
        }
    }

    Ok(())
}

/// Populate `site_packages` by recursively copying files from `source`.
fn populate_copies(site_packages: &Path, source: &Path) -> io::Result<()> {
    if !source.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(source) {
        let entry = entry?;
        let source_path = entry.path();
        let Ok(relative) = source_path.strip_prefix(source) else {
            continue;
        };
        let target_path = site_packages.join(relative);
        if entry.file_type().is_dir() {
            fs_err::create_dir_all(&target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs_err::create_dir_all(parent)?;
            }
            fs_err::copy(source_path, target_path)?;
        }
    }

    Ok(())
}

/// Determine whether a directory can be represented as a single symlink.
/// Directories that contain native extensions or are known namespace packages
/// must not be coalesced.
fn can_coalesce_directory(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        if is_namespace_package(name) {
            return false;
        }
    }

    for entry in WalkDir::new(path) {
        let Ok(entry) = entry else {
            return false;
        };
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                if matches!(ext, "so" | "dylib" | "pyd") {
                    return false;
                }
            }
        }
    }

    true
}

/// Return `true` if `name` is a known namespace-package prefix.
fn is_namespace_package(name: &str) -> bool {
    matches!(name, "google" | "zope" | "azure" | "botocore")
}
