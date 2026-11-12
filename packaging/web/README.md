# Hosting EffectCraft for the web

`effectcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `effectcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `effectcraft_web.js` | wasm-bindgen glue (generated, ES module) |
| `effectcraft_web_bg.wasm` | The app: about 58 MB, about 17 MB with gzip and 10 MB with Brotli (v0.3.1) |
| `snippets/…/js/host.js` | Browser glue (storage, audio, workers) |
| `worker.js`, `audio-worklet.js` | Render and frame workers, and audio playback |
| `sw.js`, `manifest.webmanifest`, icons | Offline install: the service worker and the app manifest |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative: the page is written that way
(`apps/effectcraft-web/web/index.html`), and `package.sh` refuses a build with root-absolute
URLs. So the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/effectcraft/`) and from a CDN bucket.

The file names are the same in every release (`effectcraft_web_bg.wasm` and so on), so browsers
must revalidate them: a long-lived cache would keep returning visitors on the old version after
you upgrade in place. Revalidation is cheap (an unchanged file answers `304 Not Modified`), and
the service worker keeps the app for offline use after the first visit. To cache the files for
longer, serve each release from its own versioned path (`/effectcraft/0.4.0/`).

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. For v0.3.1 that takes
  the download from about 58 MB to about 17 MB (gzip) or 10 MB (Brotli). You can also
  precompress (`brotli -k *.wasm`) and let the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: no-cache` (revalidate) on `index.html`, `sw.js` and the `.wasm`
  and `.js` files, since their names don't change between releases (see above).
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** EffectCraft doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /effectcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* (\.(wasm|js)|index\.html)$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Startup project (optional)

To choose what the page opens on load, add an `effectcraft-config.json` next to `index.html`:

```json
{"startup":"empty"}
```

`"empty"` restores the last visit's project and otherwise starts blank (never the demo); `"demo"`
always opens the demo project without saving the session (the last visit's project stays); `"last"` (the default) restores the last visit's project and shows
the demo on a first visit. Serve the file with `Cache-Control: no-cache`. Details:
[docs/web.md](https://github.com/storytold/effectcraft/blob/main/docs/web.md#startup-project).

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/effectcraft/"
  title="EffectCraft"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work. Preferences, projects and imported media are kept in the
  iframe origin's browser storage: the Origin Private File System, else IndexedDB, else memory
  only (`effectcraft.info().storage.backend` says which; see `docs/web.md` › Persistence).
  Browsers that partition or block third-party storage may forget them between visits, and the
  app then starts with defaults.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`. Without
  `allow-same-origin` there's no storage. Without `allow-downloads`, Save and Export (browser
  downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

EffectCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |
| `?cpu` | Force the CPU canvas path (slowest, most compatible) |

For example: `<iframe src="https://example.com/effectcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.
