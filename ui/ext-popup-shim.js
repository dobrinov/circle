// Runs in an extension's popup before its own scripts. Circle's tabs aren't
// Chrome windows, so `chrome.tabs.query` finds nothing and `chrome.tabs.create`
// has no window to open in: answer with the tab the popup was opened over,
// and open new pages as Circle tabs.
((current) => {
  if (!globalThis.chrome || !chrome.tabs) return;
  const tab = {
    active: true,
    audible: false,
    autoDiscardable: true,
    discarded: false,
    frozen: false,
    groupId: -1,
    highlighted: true,
    incognito: false,
    index: 0,
    lastAccessed: Date.now(),
    mutedInfo: { muted: false },
    pinned: false,
    selected: true,
    status: "complete",
    windowId: 1,
    ...current,
  };
  const win = {
    id: 1,
    focused: true,
    incognito: false,
    alwaysOnTop: false,
    type: "normal",
    state: "normal",
    left: 0,
    top: 0,
    width: tab.width,
    height: tab.height,
  };

  // Match patterns: <all_urls>, *://*.example.com/*, https://example.com/path*.
  const matchesPattern = (pattern, url) => {
    if (pattern === "<all_urls>") return /^(https?|wss?|ftp|file):/.test(url);
    const m = /^(\*|[a-z-]+):\/\/(\*|\*\.[^/*]+|[^/*]+)?(\/.*)$/.exec(pattern);
    if (!m) return false;
    const [, scheme, host = "", path] = m;
    let u;
    try {
      u = new URL(url);
    } catch {
      return false;
    }
    const proto = u.protocol.slice(0, -1);
    if (scheme === "*" ? !["http", "https"].includes(proto) : scheme !== proto) return false;
    if (host && host !== "*") {
      if (host.startsWith("*.")) {
        const base = host.slice(2);
        if (u.hostname !== base && !u.hostname.endsWith("." + base)) return false;
      } else if (u.hostname !== host) return false;
    }
    const glob = new RegExp("^" + path.split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$");
    return glob.test(u.pathname + u.search);
  };
  const matches = (query = {}) =>
    Object.entries(query).every(([key, value]) => {
      switch (key) {
        case "active":
        case "highlighted":
        case "currentWindow":
        case "lastFocusedWindow":
          return value === true;
        case "windowId":
          return value === 1 || value === chrome.windows?.WINDOW_ID_CURRENT;
        case "windowType":
          return value === "normal";
        case "status":
          return value === "complete";
        case "index":
          return value === 0;
        case "pinned":
        case "audible":
        case "muted":
        case "discarded":
        case "autoDiscardable":
          return value === tab[key];
        case "url":
          return (Array.isArray(value) ? value : [value]).some((p) => matchesPattern(p, tab.url));
        case "title":
          return new RegExp("^" + String(value).split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$").test(tab.title || "");
        default:
          return true;
      }
    });

  // Both styles: a callback last, or a promise.
  const api = (fn) =>
    function (...args) {
      const callback = typeof args[args.length - 1] === "function" ? args.pop() : null;
      const promise = Promise.resolve().then(() => fn(...args));
      if (!callback) return promise;
      promise.then(callback, () => callback());
    };
  const replace = (object, name, fn) => {
    if (!object) return;
    try {
      object[name] = fn;
    } catch {}
    if (object[name] !== fn) {
      try {
        Object.defineProperty(object, name, { value: fn, configurable: true, writable: true });
      } catch {}
    }
  };
  const open = (url) => {
    const target = new URL(url || "about:blank", location.href).href;
    window.open(target, "_blank");
    return { ...tab, id: chrome.tabs.TAB_ID_NONE ?? -1, active: true, url: target, pendingUrl: target, status: "loading" };
  };

  const original = {
    query: chrome.tabs.query?.bind(chrome.tabs),
    get: chrome.tabs.get?.bind(chrome.tabs),
  };
  replace(chrome.tabs, "query", api(async (query) => {
    let found = [];
    try {
      found = (await original.query(query || {})) || [];
    } catch {}
    if (found.length) return found;
    return tab.id >= 0 && matches(query || {}) ? [tab] : [];
  }));
  replace(chrome.tabs, "get", api(async (id) => (id === tab.id ? tab : original.get(id))));
  replace(chrome.tabs, "create", api(async (properties = {}) => open(properties.url)));
  if (chrome.windows) {
    const withTabs = (options) => ({ ...win, tabs: options?.populate ? [tab] : undefined });
    replace(chrome.windows, "getCurrent", api(async (options) => withTabs(options)));
    replace(chrome.windows, "getLastFocused", api(async (options) => withTabs(options)));
    replace(chrome.windows, "getAll", api(async (options) => [withTabs(options)]));
    replace(chrome.windows, "create", api(async (data = {}) => {
      open(Array.isArray(data.url) ? data.url[0] : data.url);
      return { ...win, id: 2 };
    }));
  }
  if (chrome.runtime) {
    const manifest = chrome.runtime.getManifest?.() || {};
    const options = manifest.options_ui?.page || manifest.options_page;
    replace(chrome.runtime, "openOptionsPage", api(async () => {
      if (options) open(chrome.runtime.getURL(options));
    }));
  }
})
