# EffectCraft plug-ins

The community catalog of WebAssembly effect plug-ins for
[EffectCraft](https://github.com/storytold/effectcraft). The app's Plug-in Library
(Settings ▸ Plug-in Library) reads [`index.json`](index.json) from this repository, then
downloads, checks and installs the packages people pick. Plug-ins run in EffectCraft's sandbox:
they can't read files, open the network or crash the app.

This repository holds only the catalog. Each plug-in's code and `.wasm` live in its author's
repository, attached to a GitHub release.

## Submit a plug-in

1. Build your plug-in and attach the `.wasm` to a GitHub release. The full guide is
   [Publishing plug-ins](https://github.com/storytold/effectcraft/blob/main/docs/publishing-plugins.md).
2. Add one entry to the `packages` array of `index.json`. [`example-entry.json`](example-entry.json)
   shows every field; [`schema.json`](schema.json) is the exact format. To publish a new version,
   add a release to your package's `versions` rather than editing an old one.
3. Check it locally: `scripts/validate.sh` (needs bash, jq, curl, sha256sum and Node; set
   `EFFECTCRAFT_CLI` to an `effectcraft-cli` build to also install it with the real loader).
4. Open a pull request. CI runs the same script; a maintainer reviews the rest.

## Review checklist (maintainers)

CI checks the format, unique ids, each download's size and SHA-256, and that EffectCraft installs
and loads the package. Before merging, also check:

- [ ] The id is new, stable and names the author (reverse domain style: `io.github.user.name`).
      Ids never change and are never reused.
- [ ] `repo` is a public repository with the plug-in's source, and its licence is open (the
      `license` field matches it).
- [ ] Every `url` is a release asset of that repository (not a branch file, which can change).
- [ ] The release builds from the tagged source; spot-check large or obfuscated modules.
- [ ] `effects` lists the effect ids the module registers, and they don't clash with another
      package's.
- [ ] `name`, `description` and `tags` are accurate; no Adobe names, logos or assets.
- [ ] `preview.video`, if given, is a short demo of the effect hosted on GitHub or a well-known
      video site.
- [ ] A change to an existing package comes from its author (or with their agreement), and old
      releases keep their url and SHA-256.

## Create this repository from the template

The template lives in EffectCraft at `examples/plugin-catalog/`.

1. Create the empty public repository `storytold/effectcraft-plugins` (branch `main`, so the app's
   catalog URL `https://raw.githubusercontent.com/storytold/effectcraft-plugins/main/index.json`
   resolves).
2. Copy `README.md`, `index.json`, `example-entry.json`, `schema.json` and `scripts/` to its root,
   and `validate.yml` to `.github/workflows/validate.yml`.
3. Keep `scripts/validate.sh` executable (`git update-index --chmod=+x scripts/validate.sh`).
4. Protect `main`: require pull requests and the `Validate` check.
