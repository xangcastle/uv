use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn shim_forwards_args_and_env() {
    let shim_bin = PathBuf::from(env!("CARGO_BIN_EXE_uv-bazel-shim"));

    let temp = tempfile::tempdir().unwrap();
    let venv_root = temp.path().join(".venv");
    let bin_dir = venv_root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Copy the shim into the venv bin directory.
    let shim_path = bin_dir.join("python");
    std::fs::copy(&shim_bin, &shim_path).unwrap();
    let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shim_path, perms).unwrap();

    // Build a fake runfiles tree.
    let runfiles = temp.path().join(".venv.runfiles");
    let repo_dir = runfiles.join("_main");
    let dummy_dir = repo_dir.join("python3.12").join("bin");
    std::fs::create_dir_all(&dummy_dir).unwrap();

    let dummy_python = dummy_dir.join("python3.12");
    std::fs::write(
        &dummy_python,
        "#!/bin/sh\nprintf '%s\\n' \"$@\"\nprintf 'VIRTUAL_ENV=%s\\n' \"$VIRTUAL_ENV\"\nprintf 'PYTHONEXECUTABLE=%s\\n' \"$PYTHONEXECUTABLE\"\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&dummy_python).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&dummy_python, perms).unwrap();

    // Write pyvenv.cfg.
    let pyvenv_cfg = venv_root.join("pyvenv.cfg");
    std::fs::write(
        &pyvenv_cfg,
        "home = /usr/bin\nimplementation = CPython\naspect-runfiles-interpreter = python3.12/bin/python3.12\naspect-runfiles-repo = _main\n",
    )
    .unwrap();

    // Invoke the shim.
    let output = Command::new(&shim_path)
        .arg("hello")
        .arg("world")
        .env("RUNFILES_DIR", &runfiles)
        .output()
        .expect("failed to execute shim");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "shim failed: stdout={stdout}, stderr={stderr}"
    );

    assert!(
        stdout.contains("hello"),
        "expected 'hello' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("world"),
        "expected 'world' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("VIRTUAL_ENV="),
        "expected VIRTUAL_ENV in stdout: {stdout}"
    );
    assert!(
        stdout.contains("PYTHONEXECUTABLE="),
        "expected PYTHONEXECUTABLE in stdout: {stdout}"
    );
}

#[test]
fn shim_uses_absolute_fallback() {
    let shim_bin = PathBuf::from(env!("CARGO_BIN_EXE_uv-bazel-shim"));

    let temp = tempfile::tempdir().unwrap();
    let venv_root = temp.path().join(".venv");
    let bin_dir = venv_root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let shim_path = bin_dir.join("python");
    std::fs::copy(&shim_bin, &shim_path).unwrap();
    let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shim_path, perms).unwrap();

    let dummy = temp.path().join("fallback_python");
    std::fs::write(&dummy, "#!/bin/sh\necho 'fallback-ok'\n").unwrap();
    let mut perms = std::fs::metadata(&dummy).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&dummy, perms).unwrap();

    let pyvenv_cfg = venv_root.join("pyvenv.cfg");
    std::fs::write(
        &pyvenv_cfg,
        format!(
            "home = /usr/bin\nimplementation = CPython\naspect-runfiles-interpreter = missing/bin/python\naspect-runfiles-repo = _main\naspect-absolute-interpreter = {}\n",
            dummy.display()
        ),
    )
    .unwrap();

    let output = Command::new(&shim_path)
        .output()
        .expect("failed to execute shim");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "shim failed: {stdout}");
    assert!(
        stdout.contains("fallback-ok"),
        "expected fallback-ok: {stdout}"
    );
}

#[test]
fn shim_uses_runfiles_manifest_file() {
    let shim_bin = PathBuf::from(env!("CARGO_BIN_EXE_uv-bazel-shim"));

    let temp = tempfile::tempdir().unwrap();
    let venv_root = temp.path().join(".venv");
    let bin_dir = venv_root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let shim_path = bin_dir.join("python");
    std::fs::copy(&shim_bin, &shim_path).unwrap();
    let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shim_path, perms).unwrap();

    // Create a fake interpreter reachable only via the manifest.
    let real_python = temp.path().join("the_real_python");
    std::fs::write(&real_python, "#!/bin/sh\necho 'manifest-ok'\n").unwrap();
    let mut perms = std::fs::metadata(&real_python).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&real_python, perms).unwrap();

    let manifest = temp.path().join("MANIFEST");
    std::fs::write(
        &manifest,
        format!(
            "_main/python3.12/bin/python3.12 {}\n",
            real_python.display()
        ),
    )
    .unwrap();

    let pyvenv_cfg = venv_root.join("pyvenv.cfg");
    std::fs::write(
        &pyvenv_cfg,
        "home = /usr/bin\nimplementation = CPython\naspect-runfiles-interpreter = python3.12/bin/python3.12\naspect-runfiles-repo = _main\n",
    )
    .unwrap();

    let output = Command::new(&shim_path)
        .env("RUNFILES_MANIFEST_FILE", &manifest)
        .env_remove("RUNFILES_DIR")
        .output()
        .expect("failed to execute shim");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "shim failed: {stdout}");
    assert!(
        stdout.contains("manifest-ok"),
        "expected manifest-ok: {stdout}"
    );
}
