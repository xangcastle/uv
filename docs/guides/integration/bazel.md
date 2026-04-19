# Bazel integration

`uv` can generate Python virtual environments optimized for execution inside a Bazel sandbox. This
is useful for Bazel-based build systems (such as `aspect_rules_py`) that need ephemeral virtual
environments whose `site-packages` point into the Bazel runfiles tree rather than containing copies
of every dependency.

## Creating a Bazel-compatible virtual environment

Use the `bazel-runfiles` mode when creating a virtual environment:

```bash
uv venv \
  --mode=bazel-runfiles \
  --pth-manifest=deps.json \
  .venv
```

- `--mode=bazel-runfiles` tells `uv` to post-process the environment for Bazel.
- `--pth-manifest` is a JSON file that describes where the runfiles dependencies live.

### Manifest format

`deps.json` must contain a top-level object with the following schema:

```json
{
  "repository": "pypi",
  "python_version": "3.12",
  "entries": [
    {
      "repo": "whl_install__requests",
      "path": "install/lib/python3.12/site-packages",
      "strategy": "pth"
    },
    {
      "repo": "whl_install__numpy",
      "path": "install/lib/python3.12/site-packages",
      "strategy": "symlink"
    }
  ]
}
```

- `repository` — the name of the pip repository or hub.
- `python_version` — the Python version string (informational).
- `entries` — a list of dependency entries.
  - `repo` — the Bazel external repository name as it appears in the runfiles tree.
  - `path` — the path inside that repo to the import root.
  - `strategy` — one of:
    - `pth` — write a relative path into `_bazel.pth` inside `site-packages`. This is the fastest
      strategy and is preferred for pure-Python packages.
    - `symlink` — create relative symlinks inside `site-packages`. Used for packages with native
      extensions (`.so`, `.dylib`, `.pyd`) that need to be physically present for `dlopen`
      resolution.
    - `copy` — copy files into `site-packages`. Fallback for edge cases where the destination must
      be writable (for example, when bytecode caching is required and the source tree is read-only).

## How it works

1. `uv` creates a standard virtual environment using the requested interpreter.
2. It appends Bazel-specific keys to `pyvenv.cfg`:
   ```ini
   aspect-runfiles-interpreter = <runfiles_path_to_real_python>
   aspect-runfiles-repo = <repo_name>
   aspect-absolute-interpreter = /absolute/path/to/python3
   ```
3. It populates `site-packages` using the strategies defined in the manifest.
4. It replaces the venv Python executable with a small compiled shim chosen to match the *
   *interpreter's platform** (not the host platform). The shim reads `pyvenv.cfg`, resolves the real
   interpreter via the Bazel runfiles mechanism (`RUNFILES_DIR`, `RUNFILES_MANIFEST_FILE`, or a
   sibling `.runfiles` directory), and then invokes the interpreter. On Unix the shim uses `execve`
   with `argv[0]` spoofing so Python believes it lives inside the virtual environment; on Windows it
   launches the interpreter with `CreateProcessW`.

### Cross-platform venv creation

`uv` embeds shims for all supported platforms and selects the correct one at runtime based on the
interpreter's platform (`os`, `arch`, and `libc`). This allows you to create a Linux virtual
environment from macOS (or vice versa) as long as the interpreter path points to a Python binary for
the target platform.

For example, on macOS you can create a Linux `aarch64` venv by pointing `--python` to a Linux Python
interpreter that is available in the build sandbox:

```bash
uv venv \
  --mode=bazel-runfiles \
  --pth-manifest=deps.json \
  --python=external/python_linux_aarch64/bin/python3 \
  .venv
```

`uv` will detect that the interpreter is `aarch64-unknown-linux-gnu` and install the matching shim.

### Runfiles resolution order

The shim tries the following strategies in order:

1. `RUNFILES_DIR` — used when running inside a Bazel test or binary.
2. `RUNFILES_MANIFEST_FILE` — used on Windows and on Linux sandboxes where symlink creation is
   disabled. The manifest maps runfiles paths to local filesystem paths.
3. Sibling `.runfiles` directory — common for `bazel run`.
4. `aspect-absolute-interpreter` — absolute path fallback for non-Bazel environments such as IDEs or
   local development.

### Bytecode caching (`__pycache__`)

When `strategy: symlink` points to a read-only Bazel cache tree, Python will fail silently to write
`.pyc` files. This is harmless and expected in hermetic builds. If you need a writable destination (
for example, to warm the bytecode cache before execution), use `strategy: copy` for that entry.

### Static linking on Linux

The Unix shim is linked statically on Linux (`crt-static`) so that it can run in minimal base
images (for example, CentOS 7 or distroless containers) without requiring a modern dynamic libc.

### Windows support

The Windows shim is supported on an experimental basis. It replaces `Scripts\python.exe` (and
`pythonw.exe` if present) with the Bazel shim and resolves runfiles via `RUNFILES_MANIFEST_FILE` or
`%RUNFILES_DIR%`. Because Windows process creation does not allow in-place `execve`,
`sys.executable` may differ from the venv path in some scenarios, but `VIRTUAL_ENV` and
`PYTHONEXECUTABLE` are always set correctly.

## Adding shims for new platforms

If you need a shim for a platform that is not yet included, build the `uv-bazel-shim` crate for the
target triple and copy the binary into `crates/uv/src/commands/venv/shims/`.

### Example: adding a Linux aarch64 shim

```bash
# Using cargo-zigbuild (requires Zig)
cargo zigbuild -p uv-bazel-shim --release --target aarch64-unknown-linux-gnu

# Copy the binary to the shims directory
cp target/aarch64-unknown-linux-gnu/release/uv-bazel-shim \
   crates/uv/src/commands/venv/shims/uv-bazel-shim-aarch64-unknown-linux-gnu
```

The next time `uv` is compiled, `build.rs` will automatically discover and embed the new shim.
`bazel_postprocess.rs` will then select it when the interpreter reports a matching platform.

## Limitations

- **No seed packages** — seed packages (`pip`, `setuptools`, `wheel`) are not treated specially in
  Bazel mode. If you need them, they must be declared as regular entries in the manifest.
- **Hermetic** — the mode does not perform any network access or cache writes during
  post-processing. All file operations are confined to the declared venv path and the runfiles tree.
