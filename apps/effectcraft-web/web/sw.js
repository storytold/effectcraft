// EffectCraft service worker: the app works offline once loaded, and installs as an app.
//
// `cargo xtask web` fills in the build's file list and version. Every file is precached into a
// cache named after the version, served cache-first (responses keep the server's headers, so
// the page stays cross-origin isolated: COOP/COEP), and old versions are dropped once the new
// one activates. Anything else same-origin is fetched from the network and cached for next time.
const VERSION = "__EC_VERSION__";
const FILES = __EC_FILES__;
const CACHE = `effectcraft-${VERSION}`;

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((c) => c.addAll(FILES.map((f) => new Request(f, { cache: "reload" }))))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith("effectcraft-") && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== self.location.origin) return;
  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE).catch(() => null);
      if (cache) {
        // The page itself regardless of its query (?empty, ?demo…).
        const key = req.mode === "navigate" ? new URL("./", self.registration.scope).href : req;
        const hit = await cache.match(key, { ignoreSearch: req.mode === "navigate" }).catch(() => null);
        if (hit) return hit;
      }
      const res = await fetch(req);
      if (cache && res.ok && res.type === "basic") void cache.put(req, res.clone()).catch(() => {});
      return res;
    })(),
  );
});
