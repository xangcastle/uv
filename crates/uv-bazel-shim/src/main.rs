use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
#[cfg(any(unix, not(windows)))]
use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn main() {
    if let Err(err) = run() {
        eprintln!("uv-bazel-shim: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let argv0 = std::env::args_os()
        .next()
        .ok_or_else(|| "missing argv[0]".to_string())?;
    let exe_path = Path::new(&argv0);

    let pyvenv_cfg = find_pyvenv_cfg(exe_path)?;
    let config = parse_pyvenv_cfg(&pyvenv_cfg)?;

    let interpreter_rloc = config
        .get("aspect-runfiles-interpreter")
        .ok_or("missing aspect-runfiles-interpreter in pyvenv.cfg")?;
    let repo = config
        .get("aspect-runfiles-repo")
        .ok_or("missing aspect-runfiles-repo in pyvenv.cfg")?;

    let real_python = resolve_interpreter(exe_path, &config, repo, interpreter_rloc)?;

    let venv_root = pyvenv_cfg
        .parent()
        .and_then(|p| p.parent())
        .ok_or("invalid pyvenv.cfg location")?;

    #[cfg(unix)]
    {
        let mut cmd = Command::new(&real_python);
        cmd.arg0(&argv0);
        cmd.args(std::env::args_os().skip(1));
        cmd.env("VIRTUAL_ENV", venv_root);
        cmd.env("PYTHONEXECUTABLE", &real_python);
        cmd.env("PYTHONNOUSERSITE", "1");
        let err = cmd.exec();
        Err(format!("failed to exec {real_python:?}: {err}"))
    }

    #[cfg(windows)]
    {
        run_windows(exe_path, &real_python, venv_root)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let status = Command::new(&real_python)
            .args(std::env::args_os().skip(1))
            .env("VIRTUAL_ENV", venv_root)
            .env("PYTHONEXECUTABLE", &real_python)
            .env("PYTHONNOUSERSITE", "1")
            .status()
            .map_err(|e| format!("failed to spawn {real_python:?}: {e}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(windows)]
fn run_windows(shim_path: &Path, real_python: &Path, venv_root: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    // Build command line: shim_path as argv[0], then real args.
    let mut cmd_line: Vec<u16> = Vec::new();
    cmd_line.push('"' as u16);
    cmd_line.extend(shim_path.as_os_str().encode_wide());
    cmd_line.push('"' as u16);
    for arg in std::env::args_os().skip(1) {
        cmd_line.push(' ' as u16);
        cmd_line.push('"' as u16);
        cmd_line.extend(arg.encode_wide());
        cmd_line.push('"' as u16);
    }
    cmd_line.push(0);

    let app_name: Vec<u16> = real_python
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();

    let startup_info = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process_info = PROCESS_INFORMATION::default();

    // SAFETY: `app_name` and `cmd_line` are valid null-terminated UTF-16 strings.
    // `startup_info` is initialized with its size. `process_info` is zero-initialized.
    // CreateProcessW is called with valid pointers and handles are closed afterward.
    unsafe {
        std::env::set_var("VIRTUAL_ENV", venv_root);
        std::env::set_var("PYTHONEXECUTABLE", real_python);
        std::env::set_var("PYTHONNOUSERSITE", "1");

        let result = CreateProcessW(
            windows::core::PCWSTR(app_name.as_ptr()),
            Some(windows::core::PWSTR(cmd_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &startup_info,
            &mut process_info,
        );
        if result.is_err() {
            return Err(format!("CreateProcessW failed: {result:?}"));
        }

        let _ = CloseHandle(HANDLE(process_info.hThread.0));

        let wait_result = WaitForSingleObject(process_info.hProcess, 0xFFFFFFFF); // INFINITE
        if wait_result != WAIT_OBJECT_0 {
            return Err("WaitForSingleObject failed".to_string());
        }

        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(process_info.hProcess, &mut exit_code);
        let _ = CloseHandle(HANDLE(process_info.hProcess.0));

        std::process::exit(exit_code as i32);
    }
}

fn find_pyvenv_cfg(mut start: &Path) -> Result<PathBuf, String> {
    loop {
        let candidate = start.join("pyvenv.cfg");
        if candidate.is_file() {
            return Ok(candidate);
        }
        match start.parent() {
            Some(parent) => start = parent,
            None => return Err("pyvenv.cfg not found".to_string()),
        }
    }
}

fn parse_pyvenv_cfg(path: &Path) -> Result<HashMap<String, String>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open pyvenv.cfg: {e}"))?;
    let reader = io::BufReader::new(file);
    let mut map = HashMap::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read pyvenv.cfg: {e}"))?;
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Ok(map)
}

fn resolve_interpreter(
    exe_path: &Path,
    config: &HashMap<String, String>,
    repo: &str,
    interpreter_rloc: &str,
) -> Result<PathBuf, String> {
    // 1. RUNFILES_DIR
    if let Ok(runfiles_dir) = std::env::var("RUNFILES_DIR") {
        let candidate = PathBuf::from(runfiles_dir)
            .join(repo)
            .join(interpreter_rloc);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // 2. RUNFILES_MANIFEST_FILE
    if let Ok(manifest) = std::env::var("RUNFILES_MANIFEST_FILE") {
        if let Some(path) = resolve_from_manifest(&manifest, repo, interpreter_rloc) {
            if path.exists() {
                return Ok(path);
            }
        }
    }

    // 3. Sibling .runfiles directory
    if let Some(exe_name) = exe_path.file_name() {
        if let Some(parent) = exe_path.parent() {
            let sibling =
                parent.join(format!("{}.runfiles", exe_name.to_string_lossy()));
            let candidate = sibling.join(repo).join(interpreter_rloc);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // 4. Absolute path fallback
    if let Some(absolute) = config.get("aspect-absolute-interpreter") {
        let candidate = PathBuf::from(absolute);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "could not resolve interpreter {interpreter_rloc} for repo {repo}"
    ))
}

/// Resolve a runfiles path from a Bazel `RUNFILES_MANIFEST_FILE`.
fn resolve_from_manifest(
    manifest_path: &str,
    repo: &str,
    interpreter_rloc: &str,
) -> Option<PathBuf> {
    let target = format!("{repo}/{interpreter_rloc}");
    let file = std::fs::File::open(manifest_path).ok()?;
    let reader = io::BufReader::new(file);
    for line in reader.lines() {
        let line = line.ok()?;
        let mut parts = line.splitn(3, ' ');
        let runfiles_path = parts.next()?;
        let local_path = parts.next().filter(|p| *p != "1" && *p != "0").or_else(|| {
            parts.next()
        })?;
        if runfiles_path == target {
            return Some(PathBuf::from(local_path));
        }
    }
    None
}
