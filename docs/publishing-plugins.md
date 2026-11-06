# Publishing an effect plug-in

This guide takes you from an empty folder to an effect listed in EffectCraft's **Plug-in
Library** (Settings ▸ Plug-in Library, also Effect ▸ Plug-in Library…). The reference for the
plug-in API is [plugins.md](plugins.md#effect-plug-ins); this page is the walk-through.

If you only want to use plug-ins, skip to [For users](#for-users).

## What a plug-in is

* A plug-in is one **WebAssembly module** (a `.wasm` file) that adds one effect. It is listed in
  Effects & Presets under its category, its parameters are ordinary properties (keyframes,
  expressions, Effect Controls), and projects save it by its id.
* It runs in a **sandbox**. The module may not import anything: no files, no clock, no network,
  no random numbers. Each render gets a fuel budget, and memory is capped at 1 GiB. A plug-in that
  loops forever or traps stops with an error; it can't crash the app.
* It implements **plug-in API version 1**: a JSON manifest and five exports (`memory`,
  `ec_api_version`, `ec_manifest_ptr` / `ec_manifest_len`, `ec_alloc`, `ec_render`). The
  [ABI](plugins.md#webassembly-abi-v1) and the [contract](plugins.md#the-contract) (premultiplied
  RGBA `f32` pixels, processed in place, deterministic) are in plugins.md.
* Any language that compiles to WebAssembly without imports works. This guide uses Rust, because
  the example is in Rust.
* The Plug-in Library is in the desktop app and the CLI. The web build has no plug-ins.

## 1. Start from the example

[`examples/plugins/posterize-bands`](../examples/plugins/posterize-bands) is a complete plug-in
in plain Rust with no dependencies. Copy it into a new folder (and a new Git repository):

```sh
cp -r effectcraft/examples/plugins/posterize-bands my-effect
cd my-effect
rm -rf target
git init
```

The folder has its own `[workspace]`, so it builds on its own, outside the EffectCraft repository.

Then make it yours:

1. **`Cargo.toml`:** change `name` (the `.wasm` file is named after it, with `-` turned into
   `_`), `description`, `version` and `license`. Keep `crate-type = ["cdylib"]` and the release
   profile (`panic = "abort"` keeps the module small and import-free).
2. **The manifest** (the `MANIFEST` string in `src/lib.rs`):
   * `id`: your own, reverse-domain (`io.github.yourname.glow-rings`): letters, digits, `.`, `_`,
     `-`. It must be unique among all plug-ins, and `ec.` is reserved for built-in effects.
     **Never change it after release**: projects find the effect by this id.
   * `name`: the name shown in Effects & Presets.
   * `category`: an existing Effects & Presets category (`Stylize`, `Blur & Sharpen`, …) or a new
     one.
   * `version`: the same version as the release (`1.0.0`).
   * `author`, `description`.
   * `params`: one entry per control. Types are `slider`, `angle`, `checkbox`, `popup`, `point` and
     `color`; the fields and limits are in [The manifest](plugins.md#the-manifest).
3. **`ec_render`:** the parameters arrive as `f64`s in the order they are declared (a point takes
   two values, a colour four), then the pixels. Read them by position, change the pixels in place,
   return `0`. Check `nparams` before you read, as the example does, and return non-zero if
   something is wrong: the frame is then left unchanged.

Keep `ec_api_version`, `ec_manifest_ptr`, `ec_manifest_len` and `ec_alloc` as they are.

Things that break the sandbox rule (the module then imports something and won't load): printing
(`println!`), reading the time (`std::time`), random numbers (`getrandom`, `rand` with its default
features), and crates that need `wasm-bindgen`. If you need randomness, derive it from the pixel
position, the time and a seed parameter, so the result stays deterministic.

## 2. Build

```sh
rustup target add wasm32-unknown-unknown       # once
cargo build --release --target wasm32-unknown-unknown
```

The module is `target/wasm32-unknown-unknown/release/<name>.wasm`.

## 3. Test it in EffectCraft

Load the module straight from the build folder:

* **Desktop:** Effect ▸ Load Effect Plug-in… and pick the `.wasm`. Apply the effect to a layer,
  move every control, scrub the time, and try a 32 bpc project if your effect can see values
  above 1.
* **CLI:**

  ```sh
  effectcraft-cli exec effect.plugins.load '{"path": "target/wasm32-unknown-unknown/release/my_effect.wasm"}'
  ```

  The reply describes the effect, or says why the module was refused (a missing export, an
  import, a bad manifest, another plug-in with the same id).

To have it at every start, put the `.wasm` in the `Plug-ins` folder of the
[settings folder](#where-files-live).

## 4. Check the live preview

Every package in the Plug-in Library gets a **live preview**: you do nothing to get it. EffectCraft
downloads the module (without installing it) and renders your effect in the sandbox on a built-in
sample frame (320 × 180: a colour gradient, two discs and a band of fine stripes), with the
**default parameters**, for 2 seconds of animation (12 frames per second),
with a Before/After split.

So choose defaults that show what the effect does at a glance. An effect whose defaults do
nothing (a strength of 0, an "Enable" checkbox that is off) previews as an unchanged frame. If
your effect animates over time, the 2 seconds show it. A render that fails in the preview shows
as an error.

You can see the preview before you publish, with a catalog on your disk: see
[step 8](#8-test-the-entry-locally).

## 5. Publish a GitHub release

The source repository must be **public on GitHub**, with a licence file.

1. Commit everything, including `Cargo.lock`.
2. Tag the version (`git tag v1.0.0`) and push the tag.
3. Create a GitHub release for the tag and attach the `.wasm` as an asset.

The download URL is then
`https://github.com/<you>/<repo>/releases/download/v1.0.0/my_effect.wasm`.

**Never replace an asset after it is listed.** The catalog pins its SHA-256; a different file
fails every install. Fix a mistake with a new version.

Reviewers rebuild the module from the tagged source and compare checksums, so make the build
reproducible: commit `Cargo.lock`, pin the Rust version with a `rust-toolchain.toml`
(`[toolchain] channel = "1.95.0"`, `targets = ["wasm32-unknown-unknown"]`), and build with
`cargo build --release --locked --target wasm32-unknown-unknown`. Building the release in GitHub
Actions from the tag is the easiest way to show the asset comes from the source.

## 6. Compute the checksum and size

The catalog needs the file's SHA-256 (64 lower-case hex digits) and its size in bytes. Use the
file you attached to the release (download it again if in doubt).

| System | SHA-256 | Size in bytes |
|---|---|---|
| Linux | `sha256sum my_effect.wasm` | `stat -c %s my_effect.wasm` |
| macOS | `shasum -a 256 my_effect.wasm` | `stat -f %z my_effect.wasm` |
| Windows (PowerShell) | `(Get-FileHash my_effect.wasm -Algorithm SHA256).Hash.ToLower()` | `(Get-Item my_effect.wasm).Length` |

## 7. Write the catalog entry

The catalog is one `index.json` in the
[`storytold/effectcraft-plugins`](https://github.com/storytold/effectcraft-plugins) repository.
A starting point for that repository (an example entry, a JSON Schema and the checks run on
every pull request) is in [`examples/plugin-catalog`](../examples/plugin-catalog).

Your entry is one object in `packages`:

```json
{
  "id": "io.github.yourname.glow-rings",
  "name": "Glow Rings",
  "author": "Your Name",
  "description": "Adds soft concentric rings of light around bright areas.",
  "license": "MIT",
  "repo": "https://github.com/yourname/glow-rings",
  "tags": ["stylize", "light"],
  "preview": {"video": "https://www.youtube.com/watch?v=XXXXXXXXXXX"},
  "versions": [
    {
      "version": "1.0.0",
      "api": 1,
      "minApp": "0.17.0",
      "url": "https://github.com/yourname/glow-rings/releases/download/v1.0.0/glow_rings.wasm",
      "sha256": "<64 hex digits>",
      "size": 18342,
      "effects": ["io.github.yourname.glow-rings"]
    }
  ]
}
```

Package fields:

| Field | Required | What it is |
|---|---|---|
| `id` | yes | The package id: 1 to 100 characters, letters, digits, `.`, `-` and `_`; no leading dot and no `..`. It is also the folder name on users' disks. Unique in the catalog; usually the same as the effect id. Never change it. |
| `name` | yes | The name shown in the list. Not empty. |
| `author` | no | You, or your group. |
| `description` | no, but reviewed | One or two plain sentences that say what the effect does. |
| `license` | no, but reviewed | The licence's SPDX id (`MIT`, `Apache-2.0`, `MIT OR Apache-2.0`…). |
| `repo` | no, but reviewed | The public source repository. The details' "Open on GitHub" opens it. |
| `tags` | no | Lower-case words for the tag filter (`blur`, `stylize`, `light`, `distort`…). Reuse tags the catalog already has. |
| `preview.video` | no | An `https://` link to a short demo video on YouTube, Vimeo or GitHub. "Watch video" opens it in the browser. A link that isn't `https://` (or is longer than 500 characters) is left out. The live preview doesn't need it. |
| `versions` | yes | One entry per release, newest or oldest first. |

Release fields (one per entry in `versions`):

| Field | Required | What it is |
|---|---|---|
| `version` | yes | Dotted numbers only, at most 32 characters: `1.0.0`, `1.2`. No `v` and no suffixes such as `-beta`. |
| `api` | yes | The plug-in API the module implements: `1`. |
| `minApp` | no | The oldest EffectCraft version the release needs (`0.17.0`). Leave it out if any version with this API works. |
| `url` | yes | The release asset's `https://` URL on GitHub. |
| `sha256` | yes | The file's SHA-256, 64 hex digits. |
| `size` | no, but reviewed | The file's size in bytes. When it is given, a download of another size is refused. |
| `effects` | yes | The effect ids the module registers (its manifest's `id`). EffectCraft uses them to find the package that provides an effect a project needs. |

The rules EffectCraft applies when it reads the catalog:

* Downloads are `https://` only, from GitHub's hosts only: `github.com`,
  `raw.githubusercontent.com`, and the hosts release assets redirect to
  (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`). Any other URL is
  refused. Redirects may lead elsewhere but stay on `https://`; the SHA-256 check is what
  guarantees the file.
* The catalog may be at most 4 MB and a module at most 64 MB.
* A package with a bad id, no name, or an id another package already uses is left out; so is a
  release with a bad version, `minApp` or `sha256`, or no `url`. The rest of the catalog still
  works.
* EffectCraft installs the newest release whose `api` it implements and whose `minApp` it meets.
  When the newest release needs a newer EffectCraft, `library.list` says so (`needsNewerApp`).
* The download must match `sha256` (and `size`, if given) exactly, or it is refused.

## 8. Test the entry locally

Before you open a pull request, test the entry with a catalog on your own disk. A local catalog
(`file://`) may also name local files, so you can test before you publish the release: point
`url` at your built module, then switch to the GitHub URL.

Make a folder with an `index.json` that holds only your package:

```json
{"schema": 1, "packages": [ { "id": "io.github.yourname.glow-rings", "…": "your entry" } ]}
```

File URLs are absolute: `file:///home/me/glow-rings/index.json` on Linux and macOS,
`file:///C:/Users/me/glow-rings/index.json` on Windows. Write a space as `%20`.

Use a separate settings folder, so the test doesn't touch your real installation:

```sh
export EFFECTCRAFT_CONFIG_DIR="$PWD/ec-test"           # PowerShell: $env:EFFECTCRAFT_CONFIG_DIR = "$PWD\ec-test"
effectcraft-cli exec library.refresh '{"catalog": "file:///home/me/glow-rings/index.json", "wait": true}'
effectcraft-cli exec library.list
effectcraft-cli exec library.preview '{"id": "io.github.yourname.glow-rings", "wait": true}'
effectcraft-cli exec library.install '{"id": "io.github.yourname.glow-rings", "wait": true}'
```

* `library.refresh` reads the catalog and keeps a copy, so the next commands (separate runs of
  the CLI) see it. A malformed entry is left out and logged with the reason.
* `library.list` shows your package as users will see it (`latest`, `effects`, `needsNewerApp`).
* `library.preview` downloads the module, checks it against `size` and `sha256`, and renders the
  preview without installing anything. A wrong checksum fails here. The reply has the frame size
  (320 × 180), the frame count, the effect's manifest, and two checks: `changes` (the effect
  changes the sample frame with its defaults) and `animated` (the frames differ over time).
* `library.install` installs it into the test settings folder and loads it.

To look at the preview in the app, start the desktop app with the control channel
(`effectcraft --control 9877`, with the same `EFFECTCRAFT_CONFIG_DIR` if you set one), point it at
your catalog, then open Settings ▸ Plug-in Library and press Preview on your package:

```sh
effectcraft-cli exec library.refresh '{"catalog": "file:///home/me/glow-rings/index.json", "wait": true}' --bridge 9877
```

The Refresh button in the app goes back to the community catalog.

## 9. Open a pull request to the catalog

1. Fork [`storytold/effectcraft-plugins`](https://github.com/storytold/effectcraft-plugins).
2. Add your package to `packages` in `index.json`. Keep the file's formatting.
3. Open a pull request. Say what the effect does and link the release. A screenshot or a short
   video helps the review.

The repository runs automatic checks on every pull request (see
[`examples/plugin-catalog`](../examples/plugin-catalog)); a maintainer then reviews it by hand.

### Review checklist

Reviewers check, and you can check first:

* **Licence.** `license` is an open-source licence and the repository has its text.
* **Public source.** `repo` is public and contains the plug-in's source at the release's tag.
* **Reproducible.** The module builds from that source (the steps are in the repository's README)
  and the result has the SHA-256 in the entry.
* **No imports.** The module imports nothing; EffectCraft refuses it otherwise. `library.install`
  from a local catalog shows it.
* **Ids.** The package id and effect ids are your own, follow the rules, don't start with `ec.`,
  and `effects` matches the manifest's `id`.
* **Sensible defaults.** The effect does something visible with its defaults, so the live
  preview shows it, and no default makes it slow.
* **Description.** It says what the effect does, in plain words.
* **Behaves.** It renders without errors at different sizes and preview resolutions, and the same
  frame twice gives the same result.

## 10. Release an update

1. Change the code, raise the version in `Cargo.toml` and in the manifest, build, and publish a
   new GitHub release (a new tag; never re-use an old one).
2. Add a new object to `versions` with the new `version`, `url`, `sha256`, `size` and `effects`.
   Keep the old entries: they may stay, and users of an older EffectCraft still get a release they
   can run (set `minApp` on the new one if it needs a newer app).
3. Test it with a local catalog as in [step 8](#8-test-the-entry-locally) (`library.update`
   updates an installed package), and open a pull request.

Users see "Update" on the package. Keep the effect id and the meaning of existing parameters, so
projects made with the old version still open the same. Add parameters at the end.

## 11. Remove a plug-in

Open a pull request that deletes your package from `packages` (or one release from `versions`).
People who installed it keep their copy: it stays in their list as unlisted until they remove it.
If a release is broken or unsafe, say so in the pull request so it is removed quickly.

## For users

Settings ▸ Plug-in Library (also Effect ▸ Plug-in Library…):

* **Fetch Catalog** reads the community catalog. Nothing is downloaded before that. Refresh reads
  it again; the last one fetched is kept, so the list works offline.
* **Preview** shows a package's effect live on a sample frame (playing, with a Before/After
  slider) without installing it. The details list the effects and their parameters, and have
  "Open on GitHub" and, when the author added one, "Watch video".
* **Install** downloads the module, checks it against the catalog's size and SHA-256 and loads
  it: the effect is in Effects & Presets at once.
* **Update** (or **Update All**) installs a newer release. **Remove** deletes a package.
* **Restart after updating or removing** a package whose effects are loaded: loaded effects can't
  be unloaded, so the change takes effect at the next start, and the list says "Restart
  EffectCraft to finish".
* A project that uses an effect you don't have: `library.missing` lists the missing effects and
  the catalog packages that provide them.

The same commands work from the CLI, the control channel and MCP: `library.list`,
`library.refresh`, `library.preview`, `library.install`, `library.update`, `library.uninstall`,
`library.missing`, `library.loadInstalled` ([plugins.md](plugins.md#plug-in-library)).

### Where files live

Packages are installed in the `Plug-ins/Library` folder of the settings folder:

| System | Settings folder |
|---|---|
| macOS | `~/Library/Application Support/EffectCraft` |
| Windows | `%APPDATA%\EffectCraft` |
| Linux | `$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` |
| Portable (a `portable.txt` beside the executable) | `EffectCraftData` beside the executable |

`EFFECTCRAFT_CONFIG_DIR` overrides it. In `Plug-ins/Library`:

* `<id>/<version>/<id>.wasm`: each installed module.
* `installed.json`: what is installed, with the SHA-256 each module was installed with. At start-up
  each module is checked against it; a module that changed on disk isn't loaded (install it
  again).
* `catalog.json`: the catalog last fetched.
