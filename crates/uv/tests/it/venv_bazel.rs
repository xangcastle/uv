use anyhow::Result;
use assert_fs::prelude::*;

use uv_test::uv_snapshot;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn bazel_runfiles_missing_manifest_fails() {
    let context = uv_test::test_context_with_versions!(&["3.12"]);

    uv_snapshot!(context.filters(), context.venv()
        .arg(context.venv.as_os_str())
        .arg("--python")
        .arg("3.12")
        .arg("--mode")
        .arg("bazel-runfiles"), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: --pth-manifest is required for --mode=bazel-runfiles
    "
    );
}

#[test]
fn bazel_runfiles_creates_venv_and_pth() -> Result<()> {
    let context = uv_test::test_context_with_versions!(&["3.12"]);

    let manifest = context.temp_dir.child("deps.json");
    manifest.write_str(
        r#"{
  "repository": "pypi",
  "python_version": "3.12",
  "entries": [
    {
      "repo": "whl_install__requests",
      "path": "install/lib/python3.12/site-packages",
      "strategy": "pth"
    },
    {
      "repo": "_main",
      "path": "libs/adminactions",
      "strategy": "pth"
    }
  ]
}"#,
    )?;

    uv_snapshot!(context.filters(), context.venv()
        .arg(context.venv.as_os_str())
        .arg("--python")
        .arg("3.12")
        .arg("--mode")
        .arg("bazel-runfiles")
        .arg("--pth-manifest")
        .arg(manifest.path()), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Using CPython 3.12.[X] interpreter at: [PYTHON-3.12]
    Creating virtual environment at: .venv
    Activate with: source .venv/[BIN]/activate
    "
    );

    context.venv.assert(predicates::path::is_dir());

    // Verify pyvenv.cfg contains Bazel-specific keys.
    let pyvenv_cfg = context.venv.child("pyvenv.cfg");
    pyvenv_cfg.assert(predicates::str::contains("aspect-runfiles-interpreter"));
    pyvenv_cfg.assert(predicates::str::contains("aspect-runfiles-repo"));
    pyvenv_cfg.assert(predicates::str::contains("aspect-absolute-interpreter"));

    // Verify _bazel.pth has the expected relative paths.
    let pth_file = context
        .venv
        .child("lib")
        .child("python3.12")
        .child("site-packages")
        .child("_bazel.pth");
    pth_file.assert(predicates::path::is_file());
    pth_file.assert(predicates::str::contains(
        "../../whl_install__requests/install/lib/python3.12/site-packages"
    ));
    pth_file.assert(predicates::str::contains(
        "../../_main/libs/adminactions"
    ));

    Ok(())
}

#[test]
#[cfg(unix)]
fn bazel_runfiles_shim_replaces_python() -> Result<()> {
    let context = uv_test::test_context_with_versions!(&["3.12"]);

    let manifest = context.temp_dir.child("deps.json");
    manifest.write_str(
        r#"{
  "repository": "pypi",
  "python_version": "3.12",
  "entries": []
}"#,
    )?;

    uv_snapshot!(context.filters(), context.venv()
        .arg(context.venv.as_os_str())
        .arg("--python")
        .arg("3.12")
        .arg("--mode")
        .arg("bazel-runfiles")
        .arg("--pth-manifest")
        .arg(manifest.path()), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Using CPython 3.12.[X] interpreter at: [PYTHON-3.12]
    Creating virtual environment at: .venv
    Activate with: source .venv/[BIN]/activate
    "
    );

    let bin_python = context.venv.child("bin").child("python");
    bin_python.assert(predicates::path::is_file());

    // It should not be a symlink.
    assert!(!std::fs::symlink_metadata(bin_python.path())?.file_type().is_symlink());

    // It should be executable.
    let metadata = std::fs::metadata(bin_python.path())?;
    let permissions = metadata.permissions();
    assert!(permissions.mode() & 0o111 != 0);

    // It should have the shim bytes (size > 0 and matching the embedded binary).
    let shim_size = metadata.len();
    assert!(shim_size > 0);

    Ok(())
}

#[test]
#[cfg(unix)]
fn bazel_runfiles_shim_bytes_installed() -> Result<()> {
    let context = uv_test::test_context_with_versions!(&["3.12"]);

    let manifest = context.temp_dir.child("deps.json");
    manifest.write_str(
        r#"{
  "repository": "pypi",
  "python_version": "3.12",
  "entries": []
}"#,
    )?;

    uv_snapshot!(context.filters(), context.venv()
        .arg(context.venv.as_os_str())
        .arg("--python")
        .arg("3.12")
        .arg("--mode")
        .arg("bazel-runfiles")
        .arg("--pth-manifest")
        .arg(manifest.path()), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Using CPython 3.12.[X] interpreter at: [PYTHON-3.12]
    Creating virtual environment at: .venv
    Activate with: source .venv/[BIN]/activate
    "
    );

    let bin_python = context.venv.child("bin").child("python");
    bin_python.assert(predicates::path::is_file());

    // It should not be a symlink.
    assert!(!std::fs::symlink_metadata(bin_python.path())?.file_type().is_symlink());

    // It should be executable.
    let metadata = std::fs::metadata(bin_python.path())?;
    let permissions = metadata.permissions();
    assert!(permissions.mode() & 0o111 != 0);

    // It should contain the embedded shim magic bytes (Mach-O header on macOS, ELF on Linux).
    let bytes = std::fs::read(bin_python.path())?;
    #[cfg(target_os = "macos")]
    assert!(&bytes[..4] == b"\xcf\xfa\xed\xfe" || &bytes[..4] == b"\xfe\xed\xfa\xcf", "expected Mach-O magic bytes");
    #[cfg(target_os = "linux")]
    assert_eq!(&bytes[..4], b"\x7fELF", "expected ELF magic bytes");

    Ok(())
}
