//! This embeds a "manifest" - a special XML document - into the uv binary on Windows builds.
//!
//! It includes reasonable defaults for Windows binaries:
//! - `System` codepage to retain backwards compatibility with previous uv releases.
//!   We can set to the utf-8 codepage in a future breaking release which lets us use the
//!   *A versions of Windows API functions without utf-16 conversion.
//! - Long path awareness allows paths longer than 260 characters in Windows operations.
//!   This still requires `LongPathsEnabled` to be set in the Windows registry.
//! - Declared Windows 7-10+ compatibility to avoid legacy compat layers from being potentially
//!   applied by not specifying any. This does not imply actual Windows 7, 8.0, 8.1 support. In
//!   cases where the application is run on Windows 7, the app is treated as Windows 7 aware
//!   rather than an unspecified legacy application (e.g. Windows XP).
//! - Standard invoker execution levels for CLI applications to disable UAC virtualization.
//!
//! See <https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests>
use std::fmt::Write;

use embed_manifest::manifest::{ActiveCodePage, ExecutionLevel, Setting, SupportedOS};
use embed_manifest::{embed_manifest, empty_manifest};

use uv_static::EnvVars;

fn main() {
    if std::env::var_os(EnvVars::CARGO_CFG_WINDOWS).is_some() {
        let [major, minor, patch] = uv_version::version()
            .splitn(3, '.')
            .map(str::parse)
            .collect::<Result<Vec<u16>, _>>()
            .ok()
            .and_then(|v| v.try_into().ok())
            .expect("uv version must be in x.y.z format");
        let manifest = empty_manifest()
            .name("uv")
            .version(major, minor, patch, 0)
            .active_code_page(ActiveCodePage::System)
            // "Windows10" includes Windows 10 and 11, and Windows Server 2016, 2019 and 2022
            .supported_os(SupportedOS::Windows7..=SupportedOS::Windows10)
            .requested_execution_level(ExecutionLevel::AsInvoker)
            .long_path_aware(Setting::Enabled);
        embed_manifest(manifest).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");

    embed_bazel_shim();
}

fn embed_bazel_shim() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let shims_dir = std::path::PathBuf::from("src/commands/venv/shims");

    let mut shim_entries: Vec<(String, std::path::PathBuf)> = Vec::new();

    for entry in fs_err::read_dir(&shims_dir).expect("read shims dir") {
        let entry = entry.expect("read dir entry");
        let name = entry.file_name().into_string().unwrap();
        if let Some(rest) = name.strip_prefix("uv-bazel-shim-") {
            let target = rest.strip_suffix(".exe").unwrap_or(rest);
            shim_entries.push((target.to_string(), entry.path()));
        }
    }

    assert!(
        !shim_entries.is_empty(),
        "No Bazel shim binaries found in {}",
        shims_dir.display()
    );

    let mut content = String::new();

    for (target, path) in &shim_entries {
        let var_name = format!(
            "SHIM_BYTES_{}",
            target.replace(['-', '.'], "_").to_uppercase()
        );
        let abs_path = fs_err::canonicalize(path).unwrap_or_else(|_| path.clone());
        let _ = writeln!(
            content,
            "pub(crate) static {}: &[u8] = include_bytes!(r#\"{}\"#);\n",
            var_name,
            abs_path.display()
        );
    }

    let _ = writeln!(
        content,
        "\npub(crate) fn select_shim(target: &str) -> Option<&'static [u8]> {{"
    );
    let _ = writeln!(content, "    match target {{");
    for (target, _) in &shim_entries {
        let var_name = format!(
            "SHIM_BYTES_{}",
            target.replace(['-', '.'], "_").to_uppercase()
        );
        let _ = writeln!(content, "        \"{target}\" => Some({var_name}),");
    }
    let _ = writeln!(content, "        _ => None,");
    let _ = writeln!(content, "    }}");
    let _ = writeln!(content, "}}");

    let shim_bytes_rs = std::path::PathBuf::from(&out_dir).join("shim_bytes.rs");
    fs_err::write(&shim_bytes_rs, content).expect("write shim_bytes.rs");

    for (_, path) in &shim_entries {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
