// Den - Shared peer list cache (TTL-based, used by terminal + filer dropdowns)
// eslint-disable-next-line no-unused-vars
const PeerCache = (() => {
  const CACHE_TTL_MS = 10_000; // 10 seconds
  let cached = null;
  let cachedAt = 0;
  let inflight = null;
  let errorShown = false; // prevent toast spam on repeated failures

  /** Fetch peer list with short TTL cache. Returns array of peer objects. */
  async function get() {
    const now = Date.now();
    if (cached && now - cachedAt < CACHE_TTL_MS) return cached;

    // Deduplicate concurrent requests
    if (inflight) return inflight;

    inflight = (async () => {
      try {
        const resp = await fetch('/api/peers', { credentials: 'same-origin' });
        if (!resp.ok) {
          showError();
          return cached || [];
        }
        const data = await resp.json();
        if (!Array.isArray(data)) return cached || [];
        cached = data;
        cachedAt = Date.now();
        errorShown = false;
        return cached;
      } catch {
        showError();
        return cached || [];
      } finally {
        inflight = null;
      }
    })();

    return inflight;
  }

  function showError() {
    if (!errorShown) {
      Toast.error('Failed to fetch peer list');
      errorShown = true;
    }
  }

  /** Invalidate the cache (e.g. after peer config change). */
  function invalidate() {
    cached = null;
    cachedAt = 0;
    inflight = null;
  }

  return { get, invalidate };
})();
