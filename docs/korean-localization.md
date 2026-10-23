# Korean interface localization

Korean can be selected in Settings > General > Language > 한국어. `Match System`
recognizes `ko`, `ko-KR`, and `ko_KR` locales. The preference uses the stable value
`ko`; command ids, effect ids, property paths and project contents stay unchanged.

## Credits and scope

The menu and initial panel translations continue the work of **keeyoooo11225** in
[community PR #1419](https://github.com/storytold/effectcraft/pull/1419), which was
closed without merging. **kangyou04** reviewed the integration, completed the
current settings/panel catalog, added locale and interpolation coverage, and
preserved user-provided names in dock titles. This is a continuation, not a claim
to have originated Korean support. Ordinary Korean compositing terms are used;
these are not advertised as official Adobe translations.

The fixed menu catalog has 628 rows, and the existing translated panel/settings
catalog has 687 rows. Product names and technical identifiers remain verbatim.
Effect registry names, effect parameters, engine error messages, property names
and tutorials can still be English. This patch does not claim full-interface
translation. Missing strings deliberately fall back to English.

## Fonts and compatibility

The existing Hangul fallback probes installed system fonts. No font files or new
dependencies are added. On Windows, install/keep a Hangul-capable system font such
as Malgun Gothic; the browser build cannot access native system font discovery.
The existing MCP/control connection and command schemas do not change.

The community 0.9.2 build backports this localization to the v0.9.2 source; the
upstream PR targets current main. Fork-only branding changes and executable
packaging are kept out of the upstream localization patch.

## Verification

- Menu tests compare Korean keys with the actual menu tree on both menu platforms.
- Panel tests check source-key coverage, duplicate keys and placeholder counts.
- Korean locale resolution and preference persistence are checked separately.
- A native headless UI snapshot checks rendering using the installed Hangul font.
- Full workspace gates are recorded with the PR, including any baseline failures.
