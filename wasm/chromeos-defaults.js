// Added by the figwasm-style deploy fork, not part of upstream. On ChromeOS (or with ?chromeos)
// it sets conservative URL flags before the app starts, so a shared-memory Chromebook GPU
// is not asked to hold several WebGPU devices and several wasm heaps at once.
// Explicit flags always win; ?nochromeos turns this off.
(function () {
  try {
    var p = new URLSearchParams(location.search);
    if (p.has("nochromeos")) return;
    if (!/\bCrOS\b/.test(navigator.userAgent) && !p.has("chromeos")) return;
    var changed = false;
    function set(k, v) { if (!p.has(k)) { p.set(k, v); changed = true; } }
    // Every worker that renders on the GPU opens a WebGPU device of its own. Some Chromebook
    // GPU stacks fail to create a second context (and then lose the page's own device too), so
    // by default use exactly one device, the page's: viewer frames render on the page's thread
    // (frameworkers=0) and background jobs render on the CPU (nogpuworkers).
    // Override with ?frameworkers=N, or ?nochromeos to turn all of this off.
    set("frameworkers", "0");
    set("nogpuworkers", "");
    if (changed) {
      var q = p.toString().replace(/=(&|$)/g, "$1");
      history.replaceState(null, "", location.pathname + "?" + q + location.hash);
    }
  } catch (e) {}
})();
