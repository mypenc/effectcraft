// Added by the figwasm-style deploy fork, not part of upstream. On ChromeOS (or with ?chromeos)
// it sets conservative URL flags before the app starts, so a shared-memory Chromebook GPU
// is not asked to hold several WebGPU devices and several wasm heaps at once.
// Explicit flags always win; ?nochromeos turns this off.
(function () {
  try {
    var p = new URLSearchParams(location.search);
    if (p.has("nochromeos")) return;
    if (!/\bCrOS\b/.test(navigator.userAgent) && !p.has("chromeos")) return;
    var mem = navigator.deviceMemory || 4; // GiB, capped at 8 by the browser
    var changed = false;
    function set(k, v) { if (!p.has(k)) { p.set(k, v); changed = true; } }
    // Every frame worker is a full engine instance with its own heap and its own GPU device.
    set("frameworkers", mem <= 2 ? "0" : "1");
    // 2 GB devices: no extra GPU devices at all, workers render on the CPU.
    if (mem <= 2) set("nogpuworkers", "");
    if (changed) {
      var q = p.toString().replace(/=(&|$)/g, "$1");
      history.replaceState(null, "", location.pathname + "?" + q + location.hash);
    }
  } catch (e) {}
})();
