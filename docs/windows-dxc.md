# Windows DX12 shader compiler

Windows x86-64 MSVC builds enable wgpu's `static-dxc` feature in `effectcraft-gpu`.
wgpu 30's default `Dx12Compiler::Auto` selects this bundled compiler instead of
depending on a developer's `dxcompiler.dll` or falling back to legacy FXC. No
additional compiler DLL needs to be installed on the user's computer. This changes
shader compilation; it does not establish rendering correctness or performance.

The dependency is target-scoped. Windows ARM64 and Windows GNU keep wgpu's existing
compiler selection; macOS, Linux and WebAssembly keep their existing backends.
wgpu currently excludes static DXC on Windows ARM64. This feature does not add ARM64
DX12 compiler support or remove a backend's hardware and driver requirements.

## Build and release requirements

wgpu-hal 30.0.1 accepts `mach-dxcompiler-rs` 0.1.4 or later in the 0.1 series with
default features disabled. Release builds must lock and verify
`0.1.6+2026.09.16-48d5a66.1` (or a separately reviewed successor): this version
pins native archive SHA-256 hashes and requires an immutable GitHub release.
Older compatible versions must not be substituted without reviewing their download
verification. Commit the resolved `Cargo.lock` and build releases with `--locked`.

The build script uses `curl` and `tar` to obtain a prebuilt native library from
GitHub and checks its digest before extracting/linking it. It checks the release
API and downloads the archive whenever its build script runs, even if an archive
already exists in `OUT_DIR`. Consequently `cargo fetch`, vendoring Rust crates or
`--offline` alone does not make a clean Windows build offline. CI needs GitHub
access, these tools on `PATH`, and the MSVC toolchain required by the compiler
release (the wrapper documents Visual Studio 2022 17.11 or later). wgpu disables
the wrapper's default MSVC version check, so that check is not a compatibility
guarantee. The default dynamic-CRT native archive is approximately 26 MB compressed;
the static-CRT archive is selected when Rust flags request `+crt-static`.

The Rust wrapper is MIT-licensed; the integration offers MIT or Apache-2.0, while
DXC/LLVM/Clang retain University of Illinois/NCSA and additional permissive terms.
The exact texts and pinned revisions are in
[licenses/static-dxc/PROVENANCE.md](../licenses/static-dxc/PROVENANCE.md). Windows
packaging copies these notices into both x64 MSI and portable ZIP distributions,
and fails if a required file is missing. This is a native compiler dependency,
not a Rust implementation of DXC. NCSA is a permissive BSD-style license, separately
named here; maintainers should confirm it falls within the repository's permissive
dependency policy rather than relabeling it as BSD or MIT.

Before publishing, validate the locked Windows build on DX12 with real shader
compilation and pixel comparisons; retain Vulkan coverage and verify that ARM64,
macOS, Linux and wasm dependency graphs do not activate the native download.
For a diagnostic run, wgpu's backend options can read `WGPU_DX12_COMPILER=fxc`,
`dxc` or `static-dxc` when the application applies its environment overrides.
Do not select `static-dxc` on a build that lacks the feature.

Sources: [wgpu compiler options](https://wgpu.rs/doc/wgpu/enum.Dx12Compiler.html),
[published wrapper source](https://github.com/DouglasDwyer/mach-dxcompiler-rs/tree/77b9a2f68c146d1cf45e52ed7e7ac2457be875d5),
[native immutable release](https://github.com/DouglasDwyer/mach-dxcompiler/releases/tag/2026.09.16%2B48d5a66.1).
