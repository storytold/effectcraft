# User docs: rules for agents and contributors

Everything in this folder (except this file, CLAUDE.md and README.md) is published at
https://getartcraft.com/docs/effectcraft. The file path is the URL:
`troubleshooting/gpu-not-used.md` → `/docs/effectcraft/troubleshooting/gpu-not-used`.
Renaming a file breaks links, so add the old path to `redirect_from` when you rename.

## Who reads this

Artists using EffectCraft, Discord mods answering them, and AI assistants looking things up for
either. Write for an artist, not a developer.

## Page rules

- One question or problem per page. Lead with the answer or the fix in the first two lines.
- Plain words, short sentences, numbered steps for anything the reader does.
- Start the body at `##`. The page title comes from frontmatter.
- Link to other pages instead of repeating them. Use relative links to the `.md` file:
  `[Install EffectCraft](../getting-started/install.md)`.
- No marketing copy, no "simply", no claims you have not checked in the code, a release or an issue.
- Call the product "EffectCraft", as the app, its downloads and getartcraft.com do.
- Name UI labels exactly as the app shows them, in **bold**. Menu paths use the menu text from
  `crates/engine/src/menus.rs`.
- Shortcuts: the code writes `Cmd`, which is Ctrl on Windows and Linux. Write Ctrl for Windows.
- Windows 11 is the only Windows version to name.
- Troubleshooting pages: `## Symptoms`, `## Fix`, `## Why this happens` (optional),
  `## Still stuck?` (link the issue tracker or Discord).

## Frontmatter

    ---
    title: EffectCraft does not use my graphics card
    description: One sentence for search results, link previews and AI summaries.
    order: 10                 # optional, sorts within the folder (lower first)
    verified: 0.8.0           # optional, app version you last checked this page against
    redirect_from: [old/path] # optional, old URLs (relative to /docs/effectcraft)
    ---

Only `key: value` and `key: [a, b]` lines work. Quote values that start with a quote or contain ` #`.

Set `verified` to the latest release (the newest release tag) after checking the page against that tag with `git show <tag>:<path>`. Pages describe the latest release users download, not unreleased code on main.

## Folders

- `getting-started/` install and first run
- `guides/` how to do one thing
- `troubleshooting/` symptom → fix
- `faq/` short answers
- `reference/` lists and settings

A folder's `index.md` names the folder in the sidebar. Files or folders starting with `_` are
drafts and are not published.

## Check before you push

    node <path to your artcraft-services checkout>/frontend/apps/artcraft-website-next/scripts/docs/core.mjs docs/user

The checker lives in storytold/artcraft-services. A CI check will be added once it is on that repo's main branch.

## Support PRs

Pages written to answer a user question come in as PRs titled `docs(support): <page title>`.
Review them like code: check every claim against the source. Problems and bug reports go in the
[issue tracker](https://github.com/storytold/effectcraft/issues).
