// Shared helpers for Circle's UI pages. The browser process pushes state with
// `circle.render(state)`; pages send commands with `circle.send({...})`.
window.circle = window.circle || {};

circle.send = (message) =>
  fetch("circle://ui/api", { method: "POST", body: JSON.stringify(message) }).catch(() => {});

circle.action = (action) => circle.send({ cmd: "action", action });

circle.esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );

circle.host = (url) => {
  // Circle's own pages: circle://ui/history.html reads as "circle://history".
  const own = /^circle:\/\/ui\/([a-z-]+)/.exec(url || "");
  if (own) return `circle://${own[1]}`;
  try {
    const parsed = new URL(url);
    return parsed.hostname.replace(/^www\./, "") || url;
  } catch {
    return url || "";
  }
};

// Pages served from this machine (dev servers), which Circle marks "DEV".
// Keep in sync with `is_local_url` in controller.rs.
circle.isLocal = (url) => {
  try {
    const { protocol, hostname } = new URL(url);
    if (protocol !== "http:" && protocol !== "https:") return false;
    return (
      hostname === "localhost" ||
      hostname.endsWith(".localhost") ||
      hostname === "[::1]" ||
      hostname === "0.0.0.0" ||
      /^127(\.\d{1,3}){3}$/.test(hostname)
    );
  } catch {
    return false;
  }
};

// A tab title without the site's name ("GitHub - torvalds/linux: Linux
// kernel source tree" → "torvalds/linux"), which the address bar and the
// favicon already show. Renamed tabs keep their name.
circle.shortTitle = (tab) => {
  const full = circle.title(tab);
  if (tab.renamed) return full;
  let site = "";
  try {
    const parts = new URL(tab.url).hostname.replace(/^www\./, "").split(".");
    site = parts.length > 1 ? parts[parts.length - 2] : parts[0];
  } catch {}
  if (!site || site.length < 3) return full;
  const esc = site.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  let title = full
    .replace(new RegExp(`^${esc}\\s*[-–—:|·]\\s+`, "i"), "")
    .replace(new RegExp(`\\s+[-–—|·]\\s*${esc}(\\.\\w+)?$`, "i"), "");
  // "owner/repo: a description" → "owner/repo".
  const path = /^([\w.-]+\/[\w.-]+):\s/.exec(title);
  if (path) title = path[1];
  return title.trim() || full;
};

circle.title = (tab) => tab.title || circle.host(tab.url) || "New Tab";

circle.hash = (text) => {
  let h = 0;
  for (const ch of String(text)) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
};

const letterIcon = (label, size) =>
  `<span class="icon letter" style="--letter-hue:${circle.hash(label)};width:${size}px;height:${size}px">${circle.esc(label[0] || "•")}</span>`;

// A favicon, falling back to a colored letter when it is missing or broken.
circle.icon = (tab, size = 16) => {
  const label = circle.host(tab.url).replace(/^circle:\/\//, "") || circle.title(tab);
  // Pages that don't declare an icon usually still serve /favicon.ico. Only
  // open tabs guess it: History and Spotlight would otherwise ask every site
  // you ever visited, telling them each time.
  let src = tab.favicon;
  // Circle's pages are a secure context: plain-http icons are blocked
  // without an error event, which would leave a blank space.
  if (src && src.startsWith("http:")) src = "";
  if (!src && tab.live && /^https:/.test(tab.url || "")) {
    try {
      src = `${new URL(tab.url).origin}/favicon.ico`;
    } catch {}
  }
  if (!src) return letterIcon(label, size);
  return `<img class="icon" style="width:${size}px;height:${size}px" src="${circle.esc(src)}" data-label="${circle.esc(label)}" data-size="${size}">`;
};

document.addEventListener(
  "error",
  (event) => {
    const img = event.target;
    if (img.tagName === "IMG" && img.dataset.label !== undefined) {
      img.outerHTML = letterIcon(img.dataset.label, Number(img.dataset.size) || 16);
    }
  },
  true,
);

circle.setHue = (hue) => document.documentElement.style.setProperty("--hue", hue);

circle.matches = (query, ...fields) => {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hay = fields.join(" ").toLowerCase();
  return words.every((w) => hay.includes(w));
};
