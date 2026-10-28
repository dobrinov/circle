// Circle's Network tool. It talks to the inspected tab over Circle's local
// CDP bridge (the same one DevTools uses), sorts what the page sends into
// GraphQL operations, REST calls, live connections and page assets, and
// shows each kind the way it is worked with.
(() => {
  const $ = (id) => document.getElementById(id);
  const esc = circle.esc;
  const main = $("main");
  const MAX_ENTRIES = 3000;
  const MAX_FRAMES = 5000;
  const BODY_LIMIT = 5 * 1024 * 1024;
  const DIFF_LIMIT = 400 * 1024;

  let state = null; // what Circle last pushed: the tab, whether it is loaded, the theme
  let log = null; // the current tab's log
  const logs = new Map();
  const ui = {
    kind: "all",
    q: "",
    sel: null, // the selected group's key
    call: null, // a particular call in it (else the latest)
    tab: "main",
    op: 0, // the operation in a batch
    showAssets: false,
    preserve: false,
    view: "list", // or "replay"
    respView: "tree",
    respFilter: "",
    diff: true,
    headersAll: false,
    jwt: false,
    menu: false,
    ws: { conn: null, msg: null, q: "", dir: "both", paused: 0, tab: "payload" },
  };
  const confirmed = new Set(); // hosts the person agreed to replay changes on

  // ----- small helpers -------------------------------------------------------

  const tryJson = (text) => {
    if (typeof text !== "string") return undefined;
    const t = text.trim();
    if (!t || !"{[".includes(t[0])) return undefined;
    try {
      return JSON.parse(t);
    } catch {
      return undefined;
    }
  };
  const header = (headers, name) => {
    if (!headers) return undefined;
    const lower = name.toLowerCase();
    for (const [k, v] of Object.entries(headers)) if (k.toLowerCase() === lower) return v;
    return undefined;
  };
  const bytes = (n) => {
    if (n == null || n < 0) return "—";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  };
  const ms = (s) => (s == null ? "" : s < 1 ? `${Math.max(0, Math.round(s * 1000))} ms` : `${s.toFixed(s < 10 ? 2 : 1)} s`);
  const ago = (s) => (s < 60 ? `${Math.round(s)} s` : s < 3600 ? `${Math.floor(s / 60)}m ${Math.round(s % 60)}s` : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`);
  const short = (value, n = 140) => {
    let text;
    try {
      text = typeof value === "string" ? value : JSON.stringify(value);
    } catch {
      text = String(value);
    }
    if (text === undefined) return "";
    return text.length > n ? text.slice(0, n) + "…" : text;
  };
  // { a: 1, b: "x" } — variables and payloads read better without quoted keys.
  const loose = (value, n = 120) => short(value, n * 2).replace(/"([A-Za-z_$][\w$]*)":/g, "$1: ").replace(/,(?=\S)/g, ", ").slice(0, n);
  const utf8 = new TextDecoder();
  const fromBase64 = (b64) => {
    try {
      return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    } catch {
      return new Uint8Array();
    }
  };
  const hex = (u8, n = 16) =>
    Array.from(u8.slice(0, n), (b) => b.toString(16).padStart(2, "0")).join(" ") + (u8.length > n ? " …" : "");
  const svg = (d, size = 14) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${d}"/></svg>`;
  const ICON_BACK = svg("M15 6l-6 6 6 6");

  let toastTimer = 0;
  const toast = (text) => {
    document.querySelector(".toast")?.remove();
    const el = document.createElement("div");
    el.className = "toast";
    el.textContent = text;
    document.body.append(el);
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.remove(), 1400);
  };
  const copy = async (text, what = "Copied") => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const area = document.createElement("textarea");
      area.value = text;
      document.body.append(area);
      area.select();
      document.execCommand("copy");
      area.remove();
    }
    toast(what);
  };

  // ----- GraphQL: reading operations ----------------------------------------

  const GQL_TOKEN =
    /(#[^\n]*)|("""[\s\S]*?"""|"(?:\\.|[^"\\\n])*")|(\.\.\.)|(\$[_A-Za-z][_0-9A-Za-z]*)|(@[_A-Za-z][_0-9A-Za-z]*)|([_A-Za-z][_0-9A-Za-z]*)|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|([!&()[\]{}:=|])|(,)|(\s+)|(.)/g;
  const TOKEN_TYPES = ["com", "str", "spread", "var", "dir", "name", "num", "pun", "comma", "ws", "other"];
  const gqlTokens = (src) => {
    const out = [];
    GQL_TOKEN.lastIndex = 0;
    let m;
    while ((m = GQL_TOKEN.exec(src))) {
      const i = m.slice(1).findIndex((g) => g !== undefined);
      out.push({ t: TOKEN_TYPES[i], v: m[0] });
    }
    return out;
  };

  // The operations a document defines: [{ type, name }].
  const gqlDefinitions = (query) => {
    const defs = [];
    let depth = 0;
    let expectName = null;
    for (const tok of gqlTokens(query || "")) {
      if (tok.t === "ws" || tok.t === "com" || tok.t === "comma") continue;
      if (tok.v === "{") {
        if (depth === 0 && !expectName) defs.push({ type: "query", name: null });
        else if (depth === 0 && expectName !== "fragment-named") defs.push({ type: expectName, name: null });
        expectName = null;
        depth++;
      } else if (tok.v === "}") depth--;
      else if (depth === 0) {
        if (tok.t === "name" && ["query", "mutation", "subscription", "fragment"].includes(tok.v) && !expectName) {
          expectName = tok.v;
        } else if (tok.t === "name" && expectName && expectName !== "fragment-named") {
          defs.push({ type: expectName, name: tok.v });
          expectName = "fragment-named";
        } else if (tok.v === "(" && expectName && expectName !== "fragment-named") {
          defs.push({ type: expectName, name: null });
          expectName = "fragment-named";
        }
      }
    }
    return defs.filter((d) => d.type !== "fragment");
  };

  const MUTATION_NAME = /^(create|update|delete|remove|add|set|upsert|insert|mark|send|submit|cancel|toggle|save|reset|change|archive|publish|move|assign|invite|accept|reject|confirm|login|logout|sign)/i;

  const gqlOperation = (payload, method) => {
    const query = typeof payload.query === "string" ? payload.query : "";
    const defs = gqlDefinitions(query);
    const def = defs.find((d) => d.name && d.name === payload.operationName) || defs[0];
    const persisted = payload.extensions?.persistedQuery?.sha256Hash || null;
    const name = payload.operationName || def?.name || (persisted ? "Persisted query" : "Anonymous");
    let type = def?.type;
    let guessed = false;
    if (!type) {
      guessed = true;
      type = method === "GET" ? "query" : MUTATION_NAME.test(name) ? "mutation" : "query";
    }
    return { name, type, guessed, query, variables: payload.variables, extensions: payload.extensions, persisted, operationName: payload.operationName };
  };

  const isGqlPayload = (j) =>
    j &&
    typeof j === "object" &&
    !Array.isArray(j) &&
    ((typeof j.query === "string" && j.query.includes("{")) || !!j.extensions?.persistedQuery);

  const graphqlOf = (e) => {
    let payload = null;
    if (e.method === "GET") {
      try {
        const u = new URL(e.url);
        const q = u.searchParams.get("query");
        const ext = u.searchParams.get("extensions");
        if ((q && q.includes("{")) || (ext && ext.includes("persistedQuery"))) {
          payload = {
            query: q || undefined,
            operationName: u.searchParams.get("operationName") || undefined,
            variables: tryJson(u.searchParams.get("variables") || ""),
            extensions: tryJson(ext || ""),
          };
        }
      } catch {}
    } else if (typeof e.postData === "string") {
      if (/application\/graphql/i.test(header(e.reqHeaders, "content-type") || "")) payload = { query: e.postData };
      else {
        const j = tryJson(e.postData);
        if (isGqlPayload(j) || (Array.isArray(j) && j.length && j.every(isGqlPayload))) payload = j;
      }
    }
    if (!payload) return null;
    const list = Array.isArray(payload) ? payload : [payload];
    return { batch: Array.isArray(payload), ops: list.map((p) => gqlOperation(p, e.method)) };
  };

  // Lays a query out one field per line, however it was sent.
  const gqlFormat = (src) => {
    const toks = gqlTokens(src).filter((t) => t.t !== "ws" && t.t !== "comma");
    let out = "";
    let ind = 0;
    let paren = 0;
    let inline = 0;
    let prev = null;
    let topEnd = false;
    const newline = () => {
      out = out.replace(/ +$/, "") + "\n" + "  ".repeat(ind);
    };
    const endsValue = (p) => p && (["name", "var", "num", "str"].includes(p.t) || ["]", "}", "!"].includes(p.v));
    for (const t of toks) {
      const args = paren > 0 || inline > 0;
      if (t.t === "com") {
        if (prev) out += " ";
        out += t.v;
        newline();
        prev = null;
        continue;
      }
      if (topEnd) {
        out += "\n\n";
        topEnd = false;
        prev = null;
      }
      if (!args && t.v === "{") {
        out += (prev ? " " : "") + "{";
        ind++;
        newline();
        prev = t;
        continue;
      }
      if (!args && t.v === "}") {
        ind = Math.max(0, ind - 1);
        out = out.replace(/\s+$/, "");
        newline();
        out += "}";
        prev = t;
        if (ind === 0) topEnd = true;
        continue;
      }
      let sep = "";
      if (args) {
        if ([")", "]", "!", ":"].includes(t.v)) sep = "";
        else if (t.v === "}" || t.v === "=") sep = " ";
        else if (prev && [":", "=", "{"].includes(prev.v)) sep = " ";
        else if (prev && ["(", "["].includes(prev.v)) sep = "";
        else if (endsValue(prev)) sep = ", ";
        else sep = prev ? " " : "";
      } else if (!prev || prev.v === "{") sep = "";
      else if (["(", ")", ":", "!", "]"].includes(t.v)) sep = "";
      else if (prev.v === ":") sep = " ";
      else if (t.t === "dir") sep = " ";
      else if (prev.t === "spread") sep = t.v === "on" ? " " : "";
      else if (prev.v === "on") sep = " ";
      else if (ind > 0 && (t.t === "name" || t.t === "spread") && (endsValue(prev) || prev.v === ")")) {
        newline();
        sep = "";
      } else sep = " ";
      out += sep + t.v;
      if (t.v === "(") paren++;
      else if (t.v === ")") paren = Math.max(0, paren - 1);
      else if (args && t.v === "{") inline++;
      else if (args && t.v === "}") inline = Math.max(0, inline - 1);
      prev = t;
    }
    return out.trim();
  };

  // Colours a query; fields named in `errors` (an error's last path step)
  // are underlined.
  const gqlHighlight = (src, errors = new Set()) => {
    let html = "";
    let depth = 0;
    let paren = 0;
    let last = null; // the previous significant token
    let afterKeyword = false;
    const toks = gqlTokens(src);
    for (let i = 0; i < toks.length; i++) {
      const t = toks[i];
      const v = esc(t.v);
      if (t.t === "ws" || t.t === "comma") {
        html += t.t === "comma" ? `<span class="pun">${v}</span>` : v;
        continue;
      }
      let cls = "";
      switch (t.t) {
        case "com":
          cls = "com";
          break;
        case "str":
          cls = "str";
          break;
        case "num":
          cls = "var";
          break;
        case "var":
          cls = "var";
          break;
        case "dir":
          cls = "kw";
          break;
        case "spread":
          cls = "pun";
          break;
        case "pun":
          cls = "pun";
          break;
        case "name": {
          let next = null;
          for (let j = i + 1; j < toks.length; j++) {
            if (toks[j].t !== "ws" && toks[j].t !== "comma" && toks[j].t !== "com") {
              next = toks[j];
              break;
            }
          }
          if (depth === 0 && paren === 0 && ["query", "mutation", "subscription", "fragment"].includes(t.v)) {
            cls = "kw";
            afterKeyword = true;
          } else if (t.v === "on" && (last?.t === "spread" || (depth === 0 && last?.t === "name"))) cls = "kw";
          else if (afterKeyword && depth === 0 && paren === 0) {
            cls = "op";
            afterKeyword = false;
          } else if (last?.v === "on") cls = "ty";
          else if (paren > 0 && next?.v === ":") cls = "fld";
          else if (paren > 0 && ["true", "false", "null"].includes(t.v)) cls = "kw";
          else if (paren > 0 && (last?.v === ":" || last?.v === "[") && depth === 0) cls = "ty";
          else if (paren > 0) cls = "ty";
          else if (depth > 0) cls = errors.has(t.v) && next?.v !== ":" ? "errf" : "fld";
          break;
        }
      }
      if (t.v === "{" && paren === 0) depth++;
      else if (t.v === "}" && paren === 0) depth--;
      else if (t.v === "(") paren++;
      else if (t.v === ")") paren = Math.max(0, paren - 1);
      html += cls ? `<span class="${cls}">${v}</span>` : v;
      last = t;
    }
    return html;
  };

  // ----- REST: routes --------------------------------------------------------

  const looksLikeId = (seg) =>
    /^\d+$/.test(seg) ||
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(seg) ||
    /^[0-9a-f]{12,}$/i.test(seg) ||
    /^[A-Za-z]{1,8}_[A-Za-z0-9]{3,}$/.test(seg) ||
    (/^[A-Za-z0-9_-]{16,}$/.test(seg) && /\d/.test(seg) && /[A-Za-z]/.test(seg)) ||
    /^[^@/]+@[^@/]+\.[a-z]{2,}$/i.test(seg);
  const singular = (word) =>
    word.replace(/ies$/, "y").replace(/(ss|sh|ch|x)es$/, "$1").replace(/s$/, "").replace(/[^A-Za-z0-9]+(.)/g, (_, c) => c.toUpperCase());

  // "/api/carts/c_19a/items/it_44" → "/api/carts/:cartId/items/:itemId"
  const routeOf = (url) => {
    let u;
    try {
      u = new URL(url);
    } catch {
      return { template: url, params: [], origin: "", query: [] };
    }
    const segs = u.pathname.split("/");
    const params = [];
    const out = segs.map((seg, i) => {
      if (!seg || !looksLikeId(decodeURIComponent(seg))) return seg;
      const before = segs
        .slice(0, i)
        .reverse()
        .find((s) => s && !looksLikeId(s));
      params.push({ name: before ? singular(before) + "Id" : "id", value: decodeURIComponent(seg), index: i });
      return null;
    });
    if (params.length === 1) params[0].name = "id";
    const template = out.map((s, i) => (s === null ? ":" + params.find((p) => p.index === i).name : s)).join("/");
    return { template: template || "/", params, origin: u.origin, query: [...u.searchParams] };
  };

  const resourceOf = (template) => template.split("/").find((s) => s && !s.startsWith(":") && !/^(api|v\d+|rest|graphql)$/i.test(s)) || "/";

  // ----- the log of one tab ---------------------------------------------------

  class Log {
    constructor(tab) {
      this.tab = tab;
      this.open = false;
      this.connecting = false;
      this.entries = [];
      this.byId = new Map();
      this.conns = [];
      this.connById = new Map();
      this.pending = new Map();
      this.nextId = 1;
      this.seq = 0;
      this.retry = 0;
      this.timer = 0;
      this.mainFrame = null;
      this.replaying = [];
      this.lastTime = 0;
    }

    // Asks Circle to connect this log to its tab (again, after a failure).
    connect() {
      if (this.open || this.connecting) return;
      clearTimeout(this.timer);
      this.connecting = true;
      circle.send({ cmd: "net_connect", tab: this.tab });
    }

    // Circle connected the tab, or the connection ended.
    connected(open) {
      this.connecting = false;
      if (open && !this.open) {
        this.open = true;
        this.retry = 0;
        this.send("Network.enable", { maxPostDataSize: 1 << 20 }).catch(() => {});
        this.send("Page.getFrameTree")
          .then((r) => (this.mainFrame = r.frameTree.frame.id))
          .catch(() => {});
      } else if (!open) {
        this.open = false;
        for (const p of this.pending.values()) p.reject(new Error("The tab disconnected"));
        this.pending.clear();
        // The tab may still be starting.
        if (log === this && state?.live) this.timer = setTimeout(() => this.connect(), Math.min(5000, 400 * 2 ** this.retry++));
      }
      schedule();
    }

    send(method, params = {}) {
      return new Promise((resolve, reject) => {
        if (!this.open) return reject(new Error("Not connected to the tab"));
        const id = this.nextId++;
        this.pending.set(id, { resolve, reject });
        circle.send({ cmd: "net_send", tab: this.tab, message: JSON.stringify({ id, method, params }) });
      });
    }

    receive(m) {
      if (m.id !== undefined) {
        const p = this.pending.get(m.id);
        if (!p) return;
        this.pending.delete(m.id);
        if (m.error) p.reject(new Error(m.error.message || "Failed"));
        else p.resolve(m.result || {});
        return;
      }
      const handler = HANDLERS[m.method];
      if (handler) {
        if (typeof m.params?.timestamp === "number") this.lastTime = Math.max(this.lastTime, m.params.timestamp);
        handler.call(this, m.params || {});
        schedule();
      }
    }

    // A new page closes the old one's connections; Clear keeps open ones,
    // whose messages are still coming.
    clear(navigated) {
      this.entries = [];
      this.byId.clear();
      this.conns = navigated ? [] : this.conns.filter((c) => !c.closed);
      this.connById = new Map(this.conns.map((c) => [c.id, c]));
      for (const c of this.conns) this.add({ key: "ws:" + c.id + ":" + ++this.seq, id: c.id, kind: "live", conn: c, method: "GET", url: c.url, type: c.kind === "sse" ? "EventSource" : "WebSocket", start: c.start, status: c.status || 0 });
    }

    add(e) {
      e.seq = ++this.seq;
      e.v = 0;
      this.entries.push(e);
      if (this.entries.length > MAX_ENTRIES) {
        const gone = this.entries.splice(0, this.entries.length - MAX_ENTRIES);
        for (const g of gone) if (this.byId.get(g.id) === g) this.byId.delete(g.id);
      }
    }

    fetchBody(e) {
      if (e.bodyState) return;
      e.bodyState = "loading";
      this.send("Network.getResponseBody", { requestId: e.id })
        .then((r) => {
          e.bodyState = "done";
          if (r.base64Encoded) e.base64 = r.body;
          else e.body = r.body;
          e.json = tryJson(e.body);
          if (e.kind === "graphql") {
            const list = Array.isArray(e.json) ? e.json : [e.json];
            e.gqlErrors = list.reduce((n, r) => n + (Array.isArray(r?.errors) ? r.errors.length : 0), 0);
          }
          e.v++;
          schedule();
        })
        .catch(() => {
          e.bodyState = "gone";
          e.v++;
          schedule();
        });
    }
  }

  const classify = (e) => {
    if (e.type === "WebSocket" || e.type === "EventSource") e.kind = "live";
    else if ((e.gql = graphqlOf(e))) e.kind = "graphql";
    else if (e.type === "XHR" || e.type === "Fetch") e.kind = "rest";
    else e.kind = "asset";
    if (e.kind === "rest" || e.kind === "asset") e.route = routeOf(e.url);
  };

  const decodeEntries = (entries) => {
    if (!Array.isArray(entries) || !entries.length) return undefined;
    const parts = entries.map((x) => fromBase64(x.bytes || ""));
    const all = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const p of parts) {
      all.set(p, at);
      at += p.length;
    }
    return utf8.decode(all);
  };

  // ----- live connections: what their messages mean -------------------------

  const HEARTBEAT = /^(ping|pong|heartbeat|hb|keepalive|keep-alive|ka)$/i;

  const opNameOf = (payload) => {
    if (!payload) return null;
    if (payload.operationName) return payload.operationName;
    const def = gqlDefinitions(payload.query || "")[0];
    return def?.name || null;
  };

  const sniff = (c, text, j) => {
    if (j && !Array.isArray(j) && typeof j.type === "string" && (j.type.startsWith("connection_") || j.payload?.query || ["ka", "next", "complete", "subscribe", "start", "stop", "data"].includes(j.type)))
      return "graphql-ws";
    if (Array.isArray(j) && j.length === 5 && typeof j[2] === "string" && typeof j[3] === "string") return "phoenix";
    if (j && !Array.isArray(j) && ("identifier" in j || ["welcome", "confirm_subscription", "ping"].includes(j.type)) && /cable/.test(c.url)) return "actioncable";
    if (j && !Array.isArray(j) && "identifier" in j) return "actioncable";
    if (!j && /^[0-6](\d|\[|\{|\/|$)/.test(text) && /socket\.io|EIO=/.test(c.url)) return "socket.io";
    return null;
  };

  const PROTOCOL_NAMES = {
    "graphql-ws": "graphql-ws",
    "socket.io": "Socket.IO",
    phoenix: "Phoenix channels",
    actioncable: "Action Cable",
    sse: "Server-sent events",
  };

  const decodeFrame = (c, m) => {
    if (m.binary) {
      m.type = "binary";
      m.preview = `${m.size} bytes · ${hex(m.binary)}`;
      m.key = "binary";
      return;
    }
    const text = m.text;
    let j = tryJson(text);
    if (!c.proto && c.kind === "ws") c.proto = sniff(c, text, j);
    switch (c.proto) {
      case "graphql-ws": {
        const type = String(j?.type ?? "text");
        m.type = type;
        m.id = j?.id ?? null;
        m.heartbeat = ["ka", "ping", "pong"].includes(type);
        m.json = j?.payload !== undefined ? j.payload : j;
        if (type === "subscribe" || type === "start") {
          const name = opNameOf(j.payload);
          if (m.id != null && name) c.ops[m.id] = name;
          m.preview = `${name ? "" : "Anonymous "}${j.payload?.variables ? loose(j.payload.variables, 90) : ""}`;
          m.title = name ? `subscribe ${name}` : "subscribe";
        } else if (type === "next" || type === "data") {
          m.preview = loose(j.payload?.data ?? j.payload, 140);
          m.error = Array.isArray(j.payload?.errors) && j.payload.errors.length > 0;
        } else if (type === "error") {
          m.error = true;
          m.preview = short((Array.isArray(j.payload) ? j.payload[0]?.message : j.payload?.message) ?? j.payload, 140);
        } else if (type === "complete" || type === "stop") m.preview = "ended";
        else m.preview = j?.payload ? loose(j.payload, 120) : "";
        if (m.id != null && c.ops[m.id]) m.op = c.ops[m.id];
        break;
      }
      case "socket.io": {
        const engine = text[0];
        if (engine === "2" || engine === "3") {
          m.type = engine === "2" ? "ping" : "pong";
          m.heartbeat = true;
          m.preview = "";
          break;
        }
        if (engine === "0") {
          m.type = "open";
          m.json = tryJson(text.slice(1));
          m.preview = loose(m.json, 120);
          break;
        }
        if (engine !== "4") {
          m.type = { 1: "close", 5: "upgrade", 6: "noop" }[engine] || "packet";
          m.preview = text.slice(1);
          break;
        }
        const kind = text[1];
        let rest = text.slice(2);
        let ns = null;
        if (rest[0] === "/") {
          const comma = rest.indexOf(",");
          ns = comma < 0 ? rest : rest.slice(0, comma);
          rest = comma < 0 ? "" : rest.slice(comma + 1);
        }
        const ack = /^\d+/.exec(rest);
        if (ack) rest = rest.slice(ack[0].length);
        const data = tryJson(rest);
        if (kind === "2" && Array.isArray(data)) {
          m.type = String(data[0]);
          m.json = data.length === 2 ? data[1] : data.slice(1);
        } else {
          m.type = { 0: "connect", 1: "disconnect", 3: "ack", 4: "connect_error", 5: "binary event", 6: "binary ack" }[kind] || "message";
          m.json = data;
          m.error = kind === "4";
        }
        m.id = ns || (ack ? ack[0] : null);
        m.preview = m.json === undefined ? "" : loose(m.json, 140);
        break;
      }
      case "phoenix": {
        let topic, event, payload;
        if (Array.isArray(j)) [, , topic, event, payload] = j;
        else if (j) ({ topic, event, payload } = j);
        m.type = String(event ?? "message");
        m.id = topic ?? null;
        m.json = payload;
        m.heartbeat = topic === "phoenix" && (event === "heartbeat" || event === "phx_reply");
        m.error = payload?.status === "error" || event === "phx_error";
        m.preview = `${topic ?? ""} ${payload && Object.keys(payload).length ? loose(payload, 120) : ""}`;
        break;
      }
      case "actioncable": {
        const type = j?.type || (j?.command ? j.command : "message");
        m.type = String(type);
        m.heartbeat = type === "ping";
        let channel = null;
        try {
          const id = JSON.parse(j?.identifier || "null");
          channel = id?.channel || null;
        } catch {}
        m.id = channel;
        m.json = j?.message !== undefined ? j.message : j?.data !== undefined ? tryJson(j.data) ?? j.data : j;
        m.preview = m.heartbeat ? "" : `${channel ? channel + " " : ""}${loose(m.json, 120)}`;
        m.error = type === "reject_subscription";
        break;
      }
      default: {
        if (j && typeof j === "object" && !Array.isArray(j)) {
          const type = j.type ?? j.event ?? j.action ?? j.op ?? j.kind ?? j.method ?? j.t ?? "json";
          m.type = String(type);
          m.id = j.id ?? j.channel ?? j.topic ?? j.stream ?? null;
          m.json = j;
          m.heartbeat = HEARTBEAT.test(m.type);
          m.error = /error|fail/i.test(m.type) || !!j.error;
        } else if (j !== undefined) {
          m.type = "json";
          m.json = j;
        } else {
          m.type = c.kind === "sse" ? m.type || "message" : "text";
          m.heartbeat = HEARTBEAT.test(text.trim());
        }
        m.preview = m.json !== undefined ? loose(m.json, 140) : short(text, 140);
      }
    }
    if (m.id !== null && m.id !== undefined) m.id = String(m.id);
    m.key = m.type + "|" + (m.id ?? "");
  };

  const addFrame = (c, dir, time, payload) => {
    const m = { n: ++c.n, dir, t: time, size: 0 };
    if (payload.opcode === 2) {
      m.binary = fromBase64(payload.payloadData || "");
      m.size = m.binary.length;
    } else {
      m.text = payload.payloadData ?? "";
      m.size = new Blob([m.text]).size;
    }
    if (payload.type) m.type = payload.type;
    decodeFrame(c, m);
    c.frames.push(m);
    if (c.frames.length > MAX_FRAMES) c.frames.splice(0, c.frames.length - MAX_FRAMES);
    if (dir === "out") c.sent++;
    else c.recv++;
    c.last = time;
  };

  const newConn = (log, id, url, kind, start) => {
    const c = { id, url, kind, start: start || log.lastTime, frames: [], n: 0, sent: 0, recv: 0, ops: {}, proto: kind === "sse" ? "sse" : null, closed: false, open: kind === "sse" };
    try {
      const u = new URL(url);
      if (/socket\.io|EIO=/.test(url)) c.proto = "socket.io";
      else if (/vsn=\d/.test(u.search) || /\/socket\/websocket$/.test(u.pathname)) c.proto = "phoenix";
      else if (/\/cable\/?$/.test(u.pathname)) c.proto = "actioncable";
    } catch {}
    log.conns.push(c);
    log.connById.set(id, c);
    return c;
  };

  const HANDLERS = {
    "Network.requestWillBeSent"(p) {
      const old = this.byId.get(p.requestId);
      if (old && p.redirectResponse) {
        old.status = p.redirectResponse.status;
        old.statusText = p.redirectResponse.statusText;
        old.resHeaders = p.redirectResponse.headers;
        old.end = p.timestamp;
        old.redirect = true;
        old.key = old.key + ":" + old.seq;
        old.v++;
      }
      if (p.type === "Document" && p.frameId && p.frameId === this.mainFrame && p.requestId === p.loaderId && !p.redirectResponse && !ui.preserve) {
        this.clear(true);
        if (log === this) {
          ui.sel = null;
          ui.call = null;
          ui.view = "list";
          ui.ws.conn = null;
          ui.ws.msg = null;
        }
      }
      const r = p.request || {};
      const e = {
        key: p.requestId,
        id: p.requestId,
        method: r.method || "GET",
        url: r.url + (r.urlFragment || ""),
        type: p.type || "Other",
        start: p.timestamp,
        wall: p.wallTime,
        end: null,
        status: 0,
        reqHeaders: r.headers || {},
        postData: r.postData ?? decodeEntries(r.postDataEntries),
        initiator: p.initiator,
      };
      classify(e);
      const i = this.replaying.findIndex((x) => x.url === e.url && x.method === e.method);
      if (i >= 0) {
        e.replay = true;
        this.replaying.splice(i, 1);
      }
      if (e.type === "EventSource") e.conn = this.connById.get(p.requestId) || newConn(this, p.requestId, e.url, "sse", p.timestamp);
      this.byId.set(p.requestId, e);
      this.add(e);
      if (r.hasPostData && e.postData === undefined) {
        this.send("Network.getRequestPostData", { requestId: p.requestId })
          .then((x) => {
            e.postData = x.postData;
            classify(e);
            e.v++;
            schedule();
          })
          .catch(() => {});
      }
    },
    "Network.requestServedFromCache"(p) {
      const e = this.byId.get(p.requestId);
      if (e) e.fromCache = true;
    },
    "Network.responseReceived"(p) {
      const e = this.byId.get(p.requestId);
      if (!e) return;
      const r = p.response || {};
      e.status = r.status;
      e.statusText = r.statusText;
      e.resHeaders = r.headers || {};
      e.mime = r.mimeType;
      e.timing = r.timing;
      e.protocol = r.protocol;
      e.remote = r.remoteIPAddress ? `${r.remoteIPAddress}:${r.remotePort}` : "";
      e.fromCache = e.fromCache || r.fromDiskCache || r.fromPrefetchCache;
      e.fromSW = r.fromServiceWorker;
      if (p.type && p.type !== e.type) {
        e.type = p.type;
        classify(e);
      }
      if (e.conn) e.conn.status = r.status;
      e.v++;
    },
    "Network.loadingFinished"(p) {
      const e = this.byId.get(p.requestId);
      if (!e) return;
      e.end = p.timestamp;
      e.size = p.encodedDataLength;
      e.v++;
      if (e.conn) e.conn.closed = true;
      if ((e.kind === "graphql" || e.kind === "rest") && e.size < BODY_LIMIT && !(e.status >= 300 && e.status < 400)) this.fetchBody(e);
    },
    "Network.loadingFailed"(p) {
      const e = this.byId.get(p.requestId);
      if (!e) return;
      e.end = p.timestamp;
      e.failed = p.canceled ? "Cancelled" : p.blockedReason ? `Blocked (${p.blockedReason})` : p.corsErrorStatus ? `CORS: ${p.corsErrorStatus.corsError}` : p.errorText || "Failed";
      if (e.conn) {
        e.conn.closed = true;
        e.conn.error = e.failed;
      }
      e.v++;
    },
    "Network.eventSourceMessageReceived"(p) {
      const c = this.connById.get(p.requestId);
      if (!c) return;
      addFrame(c, "in", p.timestamp, { opcode: 1, payloadData: p.data, type: p.eventName || "message" });
      const m = c.frames[c.frames.length - 1];
      if (p.eventId) m.eventId = p.eventId;
    },
    "Network.webSocketCreated"(p) {
      const c = newConn(this, p.requestId, p.url, "ws");
      const e = { key: "ws:" + p.requestId, id: p.requestId, kind: "live", conn: c, method: "GET", url: p.url, type: "WebSocket", start: this.lastTime, status: 0, initiator: p.initiator };
      this.byId.set(p.requestId, e);
      this.add(e);
    },
    "Network.webSocketWillSendHandshakeRequest"(p) {
      const c = this.connById.get(p.requestId);
      if (!c) return;
      c.start = p.timestamp;
      c.wall = p.wallTime;
      c.reqHeaders = p.request?.headers || {};
      const e = this.byId.get(p.requestId);
      if (e) {
        e.start = p.timestamp;
        e.reqHeaders = c.reqHeaders;
      }
    },
    "Network.webSocketHandshakeResponseReceived"(p) {
      const c = this.connById.get(p.requestId);
      if (!c) return;
      c.open = true;
      c.status = p.response?.status;
      c.resHeaders = p.response?.headers || {};
      const sub = String(header(c.resHeaders, "sec-websocket-protocol") || "").toLowerCase();
      if (sub.includes("graphql")) c.proto = "graphql-ws";
      else if (sub.includes("actioncable")) c.proto = "actioncable";
      c.subprotocol = sub;
      const e = this.byId.get(p.requestId);
      if (e) {
        e.status = c.status;
        e.resHeaders = c.resHeaders;
        e.v++;
      }
    },
    "Network.webSocketFrameSent"(p) {
      const c = this.connById.get(p.requestId);
      if (c) addFrame(c, "out", p.timestamp, p.response || {});
    },
    "Network.webSocketFrameReceived"(p) {
      const c = this.connById.get(p.requestId);
      if (c) addFrame(c, "in", p.timestamp, p.response || {});
    },
    "Network.webSocketFrameError"(p) {
      const c = this.connById.get(p.requestId);
      if (c) c.error = p.errorMessage;
    },
    "Network.webSocketClosed"(p) {
      const c = this.connById.get(p.requestId);
      if (!c) return;
      c.closed = true;
      c.end = p.timestamp;
      const e = this.byId.get(p.requestId);
      if (e) {
        e.end = p.timestamp;
        e.v++;
      }
    },
  };

  // ----- filtering and grouping ---------------------------------------------

  const problem = (e) => !!e.failed || e.status >= 400 || e.gqlErrors > 0 || !!e.conn?.error;

  const parseQuery = (q) => {
    const f = { words: [], status: null, method: null, errors: false };
    for (const word of q.trim().split(/\s+/).filter(Boolean)) {
      const m = /^(status|method|has|is):(.+)$/i.exec(word);
      if (!m) f.words.push(word.toLowerCase());
      else if (m[1].toLowerCase() === "status") f.status = m[2].toLowerCase();
      else if (m[1].toLowerCase() === "method") f.method = m[2].toUpperCase();
      else if (/^(errors?|problems?|failed)$/i.test(m[2])) f.errors = true;
    }
    return f;
  };
  const statusMatches = (status, spec) => {
    if (!spec) return true;
    const s = String(status || 0);
    if (/^\dxx$/.test(spec)) return s[0] === spec[0] && s.length === 3;
    const cmp = /^(>=|<=|>|<)(\d+)$/.exec(spec);
    if (cmp) return { ">=": status >= +cmp[2], "<=": status <= +cmp[2], ">": status > +cmp[2], "<": status < +cmp[2] }[cmp[1]];
    return s === spec;
  };
  const visible = (e, f) => {
    if (f.status && !statusMatches(e.status, f.status)) return false;
    if (f.method && e.method !== f.method) return false;
    if (f.errors && !problem(e)) return false;
    if (!f.words.length) return true;
    const hay = [e.url, e.method, e.status, e.type, ...(e.gql?.ops || []).flatMap((o) => [o.name, o.type]), e.route?.template].join(" ").toLowerCase();
    return f.words.every((w) => hay.includes(w));
  };

  const varsKey = (e) => {
    try {
      return JSON.stringify(e.gql.ops.map((o) => [o.name, o.variables]));
    } catch {
      return "";
    }
  };
  const groupKey = (e, kind) => {
    if (kind === "graphql") return e.gql.batch ? "gb|" + e.gql.ops.map((o) => o.name).join("+") : "g|" + e.gql.ops[0].type + "|" + e.gql.ops[0].name;
    if (kind === "rest") return "r|" + e.method + "|" + e.route.origin + e.route.template;
    if (e.kind === "graphql") return "ga|" + e.url + "|" + varsKey(e);
    if (e.kind === "rest") return "ra|" + e.method + "|" + e.url;
    return "x|" + e.key;
  };

  // The rows to show: each a group of calls (repeats fold into one).
  const groupsFor = (l) => {
    const f = parseQuery(ui.q);
    const kind = ui.kind;
    const groups = new Map();
    for (const e of l.entries) {
      if (kind !== "all" && e.kind !== kind) continue;
      if (kind === "all" && e.kind === "asset" && !ui.showAssets) continue;
      if (!visible(e, f)) continue;
      const key = groupKey(e, kind);
      let g = groups.get(key);
      if (!g) groups.set(key, (g = { key, calls: [] }));
      g.calls.push(e);
    }
    const list = [...groups.values()];
    for (const g of list) g.last = g.calls[g.calls.length - 1];
    if (kind === "all" || kind === "asset") list.sort((a, b) => a.last.seq - b.last.seq);
    else if (kind === "rest") list.sort((a, b) => resourceOf(a.last.route.template).localeCompare(resourceOf(b.last.route.template)) || a.calls[0].seq - b.calls[0].seq);
    else list.sort((a, b) => a.calls[0].seq - b.calls[0].seq);
    return list;
  };

  const polling = (calls) => {
    if (calls.length < 3) return false;
    const gaps = [];
    for (let i = 1; i < calls.length; i++) gaps.push(calls[i].start - calls[i - 1].start);
    const mean = gaps.reduce((a, b) => a + b, 0) / gaps.length;
    const dev = Math.sqrt(gaps.reduce((a, b) => a + (b - mean) ** 2, 0) / gaps.length);
    return mean > 0.3 && dev < mean * 0.3;
  };
  const avg = (calls) => {
    const done = calls.filter((c) => c.end);
    return done.length ? done.reduce((n, c) => n + (c.end - c.start), 0) / done.length : null;
  };

  // ----- pieces of the list ---------------------------------------------------

  const METHOD_CLASS = { GET: "b-rest", HEAD: "b-rest", POST: "b-post", PUT: "b-live", PATCH: "b-live", DELETE: "b-err" };
  const ASSET_NAMES = { Document: "doc", Stylesheet: "css", Image: "img", Media: "media", Font: "font", Script: "js", TextTrack: "vtt", Manifest: "manifest", Ping: "ping", Preflight: "preflight", CSPViolationReport: "csp", Prefetch: "prefetch", SignedExchange: "sxg", Other: "other", XHR: "xhr", Fetch: "fetch" };

  const badgeOf = (e) => {
    if (e.kind === "graphql") {
      if (e.gql.batch) return `<span class="badge b-gql">batch · ${e.gql.ops.length}</span>`;
      const op = e.gql.ops[0];
      return `<span class="badge ${op.type === "subscription" ? "b-live" : "b-gql"}">${op.type}</span>`;
    }
    if (e.kind === "rest") return `<span class="badge ${METHOD_CLASS[e.method] || "b-plain"}">${esc(e.method)}</span>`;
    if (e.kind === "live") return `<span class="badge b-live">${e.type === "EventSource" ? "sse" : "ws"}</span>`;
    return `<span class="badge b-plain">${ASSET_NAMES[e.type] || esc(e.type.toLowerCase())}</span>`;
  };

  const pathHtml = (e, withQuery = true) => {
    const r = e.route || routeOf(e.url);
    let u;
    try {
      u = new URL(e.url);
    } catch {
      return esc(e.url);
    }
    const segs = u.pathname.split("/");
    const ids = new Set(r.params.map((p) => p.index));
    let html = segs.map((s, i) => (ids.has(i) ? `<span class="p">${esc(s)}</span>` : esc(s))).join("/");
    if (withQuery && u.search) {
      html += [...u.searchParams]
        .map(([k, v], i) => `<span class="q">${i ? "&amp;" : "?"}${esc(k)}=</span><span class="p">${esc(short(v, 40))}</span>`)
        .join("");
    }
    return html;
  };
  const templateHtml = (template) =>
    esc(template).replace(/:([A-Za-z]\w*)/g, '<span class="p">:$1</span>');

  // A REST error's message, from the usual places in its body.
  const restMessage = (e) => {
    const j = e.json;
    if (!j || typeof j !== "object") return "";
    const pick = (v) => (typeof v === "string" ? v : "");
    const first = Array.isArray(j.errors) ? j.errors[0] : j.errors && typeof j.errors === "object" ? Object.entries(j.errors)[0] : null;
    if (Array.isArray(first)) return `${first[0]}: ${Array.isArray(first[1]) ? first[1][0] : first[1]}`;
    return pick(j.message) || pick(j.error?.message) || pick(j.error) || pick(j.detail) || pick(j.title) || pick(first?.message) || pick(first?.detail) || "";
  };

  const statusCell = (e) => {
    if (e.kind === "live" && e.conn) {
      const c = e.conn;
      if (c.error) return `<span class="num bad">error</span>`;
      return c.closed ? `<span class="num">closed</span>` : `<span class="num ok">open</span>`;
    }
    if (e.failed) return `<span class="num bad" title="${esc(e.failed)}">failed</span>`;
    if (!e.status) return `<span class="num">…</span>`;
    const bad = e.status >= 400 || e.gqlErrors > 0;
    return `<span class="num${bad ? " bad" : ""}">${e.fromCache && e.status === 200 ? "cache" : e.status}</span>`;
  };

  const whatHtml = (g) => {
    const e = g.last;
    const count = g.calls.length > 1 ? `<span class="pill rep">×${g.calls.length}${polling(g.calls) ? " · polling" : ""}</span>` : "";
    const replay = e.replay ? `<span class="pill rep">replay</span>` : "";
    if (e.kind === "graphql") {
      const ops = e.gql.ops;
      const errs = e.gqlErrors > 0 ? `<span class="pill err">${e.gqlErrors} error${e.gqlErrors > 1 ? "s" : ""}</span>` : "";
      if (e.gql.batch) return `<span class="what">${ops.map((o) => `<b>${esc(o.name)}</b>`).join('<span class="sub">·</span>')}${errs}${count}${replay}</span>`;
      const vars = ops[0].variables && Object.keys(ops[0].variables).length ? `<span class="sub mono">${esc(loose(ops[0].variables))}</span>` : "";
      return `<span class="what"><b>${esc(ops[0].name)}</b>${errs}${count}${replay}${vars}</span>`;
    }
    if (e.kind === "rest") {
      const msg = e.status >= 400 ? restMessage(e) : "";
      return `<span class="what"><span class="path">${pathHtml(e)}</span>${count}${replay}${msg ? `<span class="sub" style="color:var(--err)">${esc(short(msg, 90))}</span>` : ""}</span>`;
    }
    if (e.kind === "live") {
      const c = e.conn;
      const n = c ? c.frames.length : 0;
      return `<span class="what"><span class="path">${esc(hostPath(e.url))}</span><span class="sub">${esc(PROTOCOL_NAMES[c?.proto] || "")}${c?.proto ? " · " : ""}${n} message${n === 1 ? "" : "s"}</span></span>`;
    }
    let file = e.url;
    try {
      const u = new URL(e.url);
      file = u.pathname.split("/").filter(Boolean).pop() || u.host;
      return `<span class="what"><span class="path">${esc(short(file, 80))}</span><span class="sub">${esc(u.host)}</span>${count}</span>`;
    } catch {
      return `<span class="what"><span class="path">${esc(short(file, 80))}</span></span>`;
    }
  };
  const hostPath = (url) => {
    try {
      const u = new URL(url);
      return u.host + u.pathname;
    } catch {
      return url;
    }
  };

  const BAR = { graphql: "var(--bar-gql)", rest: "var(--bar-rest)", live: "var(--bar-live)", asset: "var(--bar-asset)" };
  const timelineHtml = (g, span) => {
    if (!span) return '<span class="tl"></span>';
    const [t0, t1] = span;
    const width = Math.max(t1 - t0, 0.001);
    const bars = g.calls
      .slice(-12)
      .map((e) => {
        const left = ((e.start - t0) / width) * 100;
        const end = e.end ?? (e.kind === "live" && !e.conn?.closed ? t1 : e.start);
        const w = Math.max(((end - e.start) / width) * 100, 0.6);
        const color = problem(e) ? "var(--bar-err)" : BAR[e.kind];
        return `<span style="left:${left.toFixed(2)}%;width:${w.toFixed(2)}%;background:${color}"></span>`;
      })
      .join("");
    return `<span class="tl">${bars}</span>`;
  };

  const timeCell = (g) => {
    const e = g.last;
    if (e.kind === "live") return `<span class="num">${e.conn ? ago(Math.max(0, (e.conn.closed ? e.conn.end || e.conn.last || e.start : log.lastTime) - e.start)) : ""}</span>`;
    const t = g.calls.length > 1 ? avg(g.calls) : e.end ? e.end - e.start : null;
    return `<span class="num">${t == null ? "" : ms(t)}</span>`;
  };

  const rowHtml = (g, span, sel) => {
    const e = g.last;
    return `<div class="row item${sel ? " sel" : ""}" data-group="${esc(g.key)}">${badgeOf(e)}${whatHtml(g)}${statusCell(e)}<span class="num size">${bytes(e.kind === "live" ? null : e.size)}</span>${timeCell(g)}${timelineHtml(g, span)}</div>`;
  };

  const listHtml = (groups) => {
    if (!groups.length) return "";
    const starts = log.entries.map((e) => e.start).filter((t) => t != null);
    const span = starts.length ? [Math.min(...starts), Math.max(log.lastTime, ...log.entries.map((e) => e.end || e.start))] : null;
    let html = `<div class="row head"><span>Kind</span><span>Request</span><span class="num">Status</span><span class="num size">Size</span><span class="num">Time</span><span class="tl-h size">Timeline</span></div>`;
    let section = null;
    for (const g of groups.slice(-1000)) {
      if (ui.kind === "rest") {
        const res = resourceOf(g.last.route.template);
        if (res !== section) {
          section = res;
          html += `<div class="group-head">${esc(res)}</div>`;
        }
      }
      html += rowHtml(g, span, g.key === ui.sel);
      if (g.last.kind === "graphql" && g.last.gql.batch) {
        g.last.gql.ops.forEach((o, i) => {
          const r = Array.isArray(g.last.json) ? g.last.json[i] : null;
          const n = Array.isArray(r?.errors) ? r.errors.length : 0;
          html += `<div class="row child item" data-group="${esc(g.key)}" data-op="${i}"><span class="badge b-gql">${o.type}</span><span class="what"><b>${esc(o.name)}</b>${n ? `<span class="pill err">${n} error${n > 1 ? "s" : ""}</span>` : ""}${o.variables && Object.keys(o.variables).length ? `<span class="sub mono">${esc(loose(o.variables))}</span>` : ""}</span><span></span><span class="size"></span><span></span><span class="size"></span></div>`;
        });
      }
    }
    return html;
  };

  const sideHtml = (groups) => {
    let html = "";
    let section = null;
    for (const g of groups.slice(-1000)) {
      const e = g.last;
      if (ui.kind === "rest") {
        const res = resourceOf(e.route.template);
        if (res !== section) {
          section = res;
          html += `<div class="group-head">${esc(res)}</div>`;
        }
      }
      let title;
      let end = "";
      let sub;
      const t = avg(g.calls);
      if (e.kind === "graphql") {
        title = e.gql.batch ? e.gql.ops.map((o) => esc(o.name)).join(" · ") : `<b>${esc(e.gql.ops[0].name)}</b>`;
        const errs = g.calls.reduce((n, c) => n + (c.gqlErrors || 0), 0);
        end = errs ? `<span class="pill err">${errs} error${errs > 1 ? "s" : ""}</span>` : g.calls.length > 1 ? `×${g.calls.length}` : "";
      } else if (e.kind === "rest") {
        title = `<span class="path mono" style="font-size:12px">${ui.kind === "rest" ? templateHtml(e.route.template) : pathHtml(e, false)}</span>`;
        end = e.status >= 400 || e.failed ? `<span class="num bad">${e.failed ? "failed" : e.status}</span>` : g.calls.length > 1 ? `×${g.calls.length}` : "";
      } else if (e.kind === "live") {
        title = `<span class="path mono" style="font-size:12px">${esc(hostPath(e.url))}</span>`;
      } else title = `<span class="path mono" style="font-size:12px">${esc(short(e.url.split("/").pop() || e.url, 60))}</span>`;
      sub = [t != null ? `${ms(t)}${g.calls.length > 1 ? " avg" : ""}` : "", e.size != null && e.kind !== "live" ? bytes(e.size) : "", polling(g.calls) ? "polling" : ""].filter(Boolean).join(" · ");
      html += `<div class="li${g.key === ui.sel ? " sel" : ""}" data-group="${esc(g.key)}"><div class="l1">${badgeOf(e)}${title}<span class="end">${end}</span></div>${sub ? `<div class="l2">${sub}</div>` : ""}</div>`;
    }
    return html || `<div class="note" style="padding:14px">Nothing matches.</div>`;
  };

  // ----- JSON trees -----------------------------------------------------------

  const leafHtml = (v) => {
    if (typeof v === "string") {
      const shown = v.length > 3000 ? v.slice(0, 3000) + "…" : v;
      return `<span class="v s${v.length > 120 ? " trunc" : ""}" data-copy="${esc(v)}">"${esc(shown)}"</span>`;
    }
    if (typeof v === "number") return `<span class="v n" data-copy="${v}">${v}</span>`;
    if (typeof v === "boolean") return `<span class="v b" data-copy="${v}">${v}</span>`;
    if (v === null) return `<span class="v z" data-copy="null">null</span>`;
    return `<span class="meta">${esc(String(v))}</span>`;
  };
  const leafText = (v) => (typeof v === "string" ? `"${short(v, 60)}"` : v && typeof v === "object" ? (Array.isArray(v) ? `[${v.length}]` : "{…}") : String(v));
  const same = (a, b) => {
    if (a === b) return true;
    if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
    try {
      return JSON.stringify(a) === JSON.stringify(b);
    } catch {
      return false;
    }
  };

  // ctx: { open: depth shown open, diff: compare against `prev`, errors: paths }
  const node = (k, v, ctx, depth, path, prev, mode) => {
    const label = k === null ? "" : `<span class="k">${esc(k)}</span><span class="pun">: </span>`;
    const pathText = path.join(".");
    const errTag = ctx.errors?.has(pathText) && (v === null || v === undefined) ? `<span class="tag" style="color:var(--err)">nulled by an error</span>` : "";
    if (v && typeof v === "object") {
      const isArr = Array.isArray(v);
      const keys = isArr ? v.map((_, i) => i) : Object.keys(v);
      const hasPrev = ctx.diff && mode === undefined && prev !== undefined;
      const changed = hasPrev && !same(prev, v);
      const open = depth < ctx.open || (changed && depth < 8) || errTag || ctx.allOpen;
      const meta = isArr ? `${v.length} item${v.length === 1 ? "" : "s"}` : `${keys.length} key${keys.length === 1 ? "" : "s"}`;
      let kids = "";
      const prevObj = hasPrev && prev && typeof prev === "object" ? prev : null;
      for (const key of keys.slice(0, 300)) {
        const childMode = mode ?? (prevObj && !(key in prevObj) ? "add" : undefined);
        kids += node(isArr ? String(key) : key, v[key], ctx, depth + 1, path.concat(key), prevObj ? prevObj[key] : undefined, childMode);
      }
      if (keys.length > 300) kids += `<div class="leaf meta">… ${keys.length - 300} more</div>`;
      if (prevObj && Array.isArray(prevObj) === isArr) {
        for (const key of Object.keys(prevObj)) if (!(key in v)) kids += node(key, prevObj[key], ctx, depth + 1, path.concat(key), undefined, "del");
      }
      const cls = mode ? ` class="${mode}"` : "";
      const was = hasPrev && (prev === null || typeof prev !== "object") ? ` <span class="old">${esc(leafText(prev))}</span>` : "";
      return `<details${open ? " open" : ""}${cls}><summary>${label}${was}<span class="pun">${isArr ? "[" : "{"}</span><span class="meta">${meta}</span>${changed ? '<span class="tag">changed</span>' : ""}${errTag}</summary><div class="kids">${kids}</div></details>`;
    }
    if (ctx.diff && mode === undefined && prev !== undefined && !same(prev, v)) {
      return `<div class="leaf chg">${label}<span class="old">${esc(leafText(prev))}</span>${leafHtml(v)}<span class="tag">changed</span></div>`;
    }
    const sign = mode === "add" ? '<span class="tag" style="color:var(--ok)">added</span>' : mode === "del" ? '<span class="tag" style="color:var(--err)">removed</span>' : "";
    return `<div class="leaf${mode ? " " + mode : ""}">${label}${leafHtml(v)}${sign}${errTag}</div>`;
  };

  // Keeps only what mentions `text` (in a key or a value).
  const prune = (v, text) => {
    if (v && typeof v === "object") {
      const out = Array.isArray(v) ? [] : {};
      let any = false;
      for (const [k, x] of Object.entries(v)) {
        if (k.toLowerCase().includes(text)) {
          out[k] = x;
          any = true;
          continue;
        }
        const p = prune(x, text);
        if (p !== undefined) {
          out[k] = p;
          any = true;
        }
      }
      return any ? out : undefined;
    }
    return String(v).toLowerCase().includes(text) ? v : undefined;
  };

  const treeHtml = (value, opts = {}) => {
    const ctx = { open: opts.open ?? 2, diff: opts.prev !== undefined && opts.diff !== false, errors: opts.errors, allOpen: opts.allOpen };
    let v = value;
    if (opts.filter) {
      v = prune(value, opts.filter.toLowerCase());
      if (v === undefined) return `<div class="note" style="padding:4px 0">Nothing matches “${esc(opts.filter)}”.</div>`;
      ctx.allOpen = true;
    }
    return `<div class="tree">${node(null, v, ctx, 0, [], ctx.diff ? opts.prev : undefined, undefined)}</div>`;
  };

  const bodyText = (e) => (e.body != null ? e.body : e.base64 ? `(binary, ${bytes(fromBase64(e.base64).length)})` : "");
  const pretty = (text) => {
    const j = tryJson(text);
    return j === undefined ? text : JSON.stringify(j, null, 2);
  };

  // ----- copying requests -----------------------------------------------------

  const FORBIDDEN = /^(host|content-length|cookie2?|connection|origin|referer|accept-encoding|accept-charset|user-agent|date|dnt|expect|keep-alive|te|trailer|transfer-encoding|upgrade|via|access-control-request-.*|sec-.*|proxy-.*|:.*)$/i;
  const sendableHeaders = (headers) => Object.fromEntries(Object.entries(headers || {}).filter(([k]) => !FORBIDDEN.test(k)));
  const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
  const curlOf = (req) => {
    let s = `curl ${shq(req.url)}`;
    if (req.method !== "GET" || req.body != null) s += ` \\\n  -X ${req.method}`;
    for (const [k, v] of Object.entries(req.headers || {})) if (!/^:/.test(k)) s += ` \\\n  -H ${shq(`${k}: ${v}`)}`;
    if (req.body != null && req.body !== "") s += ` \\\n  --data-raw ${shq(req.body)}`;
    return s;
  };
  const fetchOf = (req) => {
    const init = { method: req.method, headers: sendableHeaders(req.headers), credentials: "include" };
    if (req.body != null && req.body !== "") init.body = req.body;
    return `await fetch(${JSON.stringify(req.url)}, ${JSON.stringify(init, null, 2)});`;
  };
  const requestOf = (e) => ({ method: e.method, url: e.url, headers: e.reqHeaders || {}, body: e.postData ?? null });

  // ----- the detail of one call ---------------------------------------------

  const selected = () => {
    if (!log || !ui.sel) return null;
    const groups = groupsFor(log);
    const g = groups.find((x) => x.key === ui.sel);
    if (!g) return null;
    const e = g.calls.find((c) => c.key === ui.call) || g.last;
    return { g, e };
  };

  const tabsHtml = (tabs) =>
    `<div class="tabs" role="tablist">${tabs
      .map(([id, label]) => `<button role="tab" aria-selected="${ui.tab === id}" class="${ui.tab === id ? "on" : ""}" data-tab="${id}">${label}</button>`)
      .join("")}</div>`;

  const actionsHtml = (e) => {
    const canReplay = e.kind === "graphql" || e.kind === "rest";
    const menu = ui.menu
      ? `<div class="menu" role="menu">
          <button role="menuitem" data-copy-as="curl">Copy as cURL</button>
          <button role="menuitem" data-copy-as="fetch">Copy as fetch</button>
          <button role="menuitem" data-copy-as="url">Copy URL</button>
          ${e.kind === "graphql" ? `<button role="menuitem" data-copy-as="query">Copy query</button><button role="menuitem" data-copy-as="variables">Copy variables</button>` : ""}
          ${e.body != null ? `<button role="menuitem" data-copy-as="response">Copy response</button>` : ""}
        </div>`
      : "";
    return `<div class="acts">
      ${canReplay ? `<button class="btn" data-act="replay">Replay</button><button class="btn primary" data-act="edit">Edit &amp; replay</button>` : ""}
      <button class="btn" data-act="menu" aria-haspopup="menu" aria-expanded="${ui.menu}">Copy ▾</button>${menu}
    </div>`;
  };

  const backHtml = (wide) => (wide ? "" : `<button class="back" data-act="back">${ICON_BACK}All requests</button>`);

  const callsHtml = (g, e) => {
    if (g.calls.length < 2) return "";
    const calls = g.calls.slice(-12);
    return `<div class="strip"><span class="chip" style="background:none;padding-left:0">Calls</span>${calls
      .map((c) => {
        const n = g.calls.indexOf(c) + 1;
        const bad = problem(c);
        return `<button class="chip${c === e ? " on" : ""}" data-call="${esc(c.key)}" style="${bad ? "color:var(--err)" : ""}">#${n} · ${c.failed ? "failed" : c.status || "…"}${c.end ? " · " + ms(c.end - c.start) : ""}</button>`;
      })
      .join("")}${g.calls.length > 12 ? `<span class="chip">+${g.calls.length - 12} earlier</span>` : ""}</div>`;
  };

  const kvHtml = (pairs, empty = "None") =>
    pairs.length
      ? `<div class="kv">${pairs.map(([k, v]) => `<span class="k">${esc(k)}</span><span class="v">${v}</span>`).join("")}</div>`
      : `<div class="note" style="padding:0">${empty}</div>`;

  const jwtOf = (value) => {
    const m = /^Bearer\s+([\w-]+)\.([\w-]+)\.([\w-]*)$/i.exec(String(value || ""));
    if (!m) return null;
    try {
      const text = atob(m[2].replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(m[2].length / 4) * 4, "="));
      return JSON.parse(utf8.decode(Uint8Array.from(text, (c) => c.charCodeAt(0))));
    } catch {
      return null;
    }
  };

  const headerRows = (headers, decode) =>
    Object.entries(headers || {})
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([k, v]) => {
        const jwt = decode && /^authorization$/i.test(k) ? jwtOf(v) : null;
        let html = esc(short(String(v), 400));
        if (jwt) {
          html = `${esc(String(v).slice(0, 18))}… <button class="chip" data-act="jwt">${ui.jwt ? "hide" : "decode JWT"}</button>`;
          if (ui.jwt) {
            const exp = typeof jwt.exp === "number" ? new Date(jwt.exp * 1000) : null;
            html += `<div class="jwt">${exp ? `<div class="note" style="padding:0 0 4px">${exp < new Date() ? "Expired" : "Expires"} ${esc(exp.toLocaleString())}</div>` : ""}${treeHtml(jwt, { open: 1 })}</div>`;
          }
        }
        return [k, html];
      });

  const IMPORTANT = /^(authorization|content-type|accept|if-none-match|if-modified-since|x-request-id|x-correlation-id|traceparent|x-api-key|apollographql-client-name|x-apollo-operation-name|cache-control)$/i;

  const timingHtml = (e) => {
    const t = e.timing;
    if (!t || !e.end) return `<div class="note" style="padding-top:12px">${e.end ? "No timing for this one: it came from a cache or a service worker." : "Still loading."}</div>`;
    const base = t.requestTime;
    const total = e.end - e.start;
    const at = (x) => base + x / 1000 - e.start;
    const phases = [
      ["Queued", 0, base - e.start, "var(--bar-asset)"],
      t.dnsStart >= 0 && ["DNS", at(t.dnsStart), at(t.dnsEnd), "var(--bar-rest)"],
      t.connectStart >= 0 && ["Connecting", at(t.connectStart), at(t.connectEnd), "var(--bar-live)"],
      t.sslStart >= 0 && ["TLS", at(t.sslStart), at(t.sslEnd), "var(--bar-live)"],
      ["Sending", at(t.sendStart), at(t.sendEnd), "var(--bar-gql)"],
      ["Waiting (server)", at(t.sendEnd), at(t.receiveHeadersEnd), "var(--accent)"],
      ["Downloading", at(t.receiveHeadersEnd), total, "var(--ok)"],
    ].filter(Boolean);
    const span = Math.max(total, 0.001);
    return `<div class="timing">${phases
      .map(([name, a, b, color]) => {
        const left = Math.max(0, (a / span) * 100);
        const width = Math.max(0.5, ((b - a) / span) * 100);
        return `<span>${name}</span><span class="bar"><span style="left:${left.toFixed(2)}%;width:${Math.min(width, 100 - left).toFixed(2)}%;background:${color}"></span></span><span class="num">${ms(Math.max(0, b - a))}</span>`;
      })
      .join("")}<span style="font-weight:600">Total</span><span></span><span class="num" style="color:var(--fg)">${ms(total)}</span></div>
      <div class="note">${esc(e.protocol || "")}${e.remote ? " · " + esc(e.remote) : ""}${e.fromSW ? " · from service worker" : ""}</div>`;
  };

  const headersHtml = (e) => `
    <div class="sect">Request</div><div class="block">${kvHtml(headerRows(e.reqHeaders, true))}</div>
    <div class="sect">Response${e.status ? ` · ${e.status} ${esc(e.statusText || "")}` : ""}</div><div class="block">${kvHtml(headerRows(e.resHeaders), "Not received yet")}</div>`;

  const rawHtml = (e) => `
    <div class="sect">Request${e.postData != null ? ` · ${bytes(new Blob([e.postData]).size)}` : ""}</div>
    <div class="block"><pre class="code">${e.postData != null ? esc(short(pretty(e.postData), 200000)) : '<span class="com">No body</span>'}</pre></div>
    <div class="sect">Response</div>
    <div class="block"><pre class="code">${e.bodyState === "done" ? esc(short(pretty(bodyText(e)), 400000)) : `<span class="com">${bodyNote(e)}</span>`}</pre></div>`;

  const bodyNote = (e) =>
    e.bodyState === "gone"
      ? "The tab no longer has this body."
      : e.failed
        ? esc(e.failed)
        : e.size >= BODY_LIMIT
          ? `Too big to show here (${bytes(e.size)}).`
          : !e.end
            ? "Still loading…"
            : "Loading…";

  // The same call made just before, for "what changed".
  const previousCall = (g, e) => {
    const i = g.calls.indexOf(e);
    for (let j = i - 1; j >= 0; j--) if (g.calls[j].json !== undefined && g.calls[j].url === e.url) return g.calls[j];
    for (let j = i - 1; j >= 0; j--) if (g.calls[j].json !== undefined) return g.calls[j];
    return null;
  };

  const resolverTiming = (ext) => {
    const resolvers = ext?.tracing?.execution?.resolvers;
    if (!Array.isArray(resolvers) || !resolvers.length) return "";
    const total = ext.tracing.duration || Math.max(...resolvers.map((r) => r.startOffset + r.duration));
    const top = [...resolvers].sort((a, b) => b.duration - a.duration).slice(0, 8);
    return `<div class="sect">Resolver timing</div><div class="rt">${top
      .map((r) => {
        const left = (r.startOffset / total) * 100;
        const width = Math.max(0.6, (r.duration / total) * 100);
        const name = r.path.filter((p) => typeof p === "string").join(".");
        return `<span class="n" title="${esc(r.path.join("."))}">${esc(name)}</span><span class="bar"><span style="left:${left.toFixed(1)}%;width:${width.toFixed(1)}%"></span></span><span class="d">${ms(r.duration / 1e9)}</span>`;
      })
      .join("")}</div>`;
  };

  const graphqlDetail = (g, e, wide) => {
    const ops = e.gql.ops;
    const i = Math.min(ui.op, ops.length - 1);
    const op = ops[i];
    const res = Array.isArray(e.json) ? e.json[i] : e.json;
    const errors = Array.isArray(res?.errors) ? res.errors : [];
    const prev = ui.diff ? previousCall(g, e) : null;
    const prevRes = prev ? (Array.isArray(prev.json) ? prev.json[i] : prev.json) : undefined;
    const errorFields = new Set(errors.map((x) => (x.path || []).filter((p) => typeof p === "string").pop()).filter(Boolean));
    // An error nulls its field, or the nearest nullable one above it: mark
    // every step of its path.
    const errorPaths = new Set(errors.flatMap((x) => (x.path || []).map((_, i) => ["data", ...x.path.slice(0, i + 1)].join("."))));
    const client = header(e.reqHeaders, "apollographql-client-name");
    const version = header(e.reqHeaders, "apollographql-client-version");
    const trace = header(e.resHeaders, "x-request-id") || header(e.resHeaders, "x-trace-id") || header(e.resHeaders, "traceparent");
    let status;
    if (e.failed) status = `<span class="chip err">${esc(e.failed)}</span>`;
    else if (!e.status) status = `<span class="chip">Waiting…</span>`;
    else if (e.gqlErrors > 0) status = `<span class="chip err">HTTP ${e.status} · but ${e.gqlErrors} GraphQL error${e.gqlErrors > 1 ? "s" : ""}</span>`;
    else if (e.status >= 400) status = `<span class="chip err">HTTP ${e.status} ${esc(e.statusText || "")}</span>`;
    else status = `<span class="chip ok">HTTP ${e.status} · no errors</span>`;

    let body = "";
    if (ui.tab === "headers") body = headersHtml(e);
    else if (ui.tab === "timing") body = timingHtml(e);
    else if (ui.tab === "raw") body = rawHtml(e);
    else {
      const query = op.query
        ? `<pre class="code">${gqlHighlight(gqlFormat(op.query), errorFields)}</pre>`
        : `<div class="note" style="padding:0">Sent as a persisted query, so only its hash travelled:<br><span class="mono">${esc(op.persisted || "")}</span></div>`;
      const vars = op.variables && Object.keys(op.variables).length ? treeHtml(op.variables, { open: 3 }) : `<div class="note" style="padding:0">None</div>`;
      const errCards = errors
        .map(
          (x) => `<div class="errcard"><div class="t"><b>${esc(x.message || "Error")}</b>${x.extensions?.code ? `<span class="code">${esc(x.extensions.code)}</span>` : ""}</div>${x.path ? `<div class="path">${esc(x.path.join(" › "))}</div>` : ""}${x.locations?.length ? `<div class="path" style="color:var(--muted)">line ${x.locations[0].line}, column ${x.locations[0].column}</div>` : ""}</div>`,
        )
        .join("");
      let response;
      if (res !== undefined) {
        response = `${errCards}<div class="block">${treeHtml(res.data !== undefined ? { data: res.data, ...(res.extensions ? { extensions: res.extensions } : {}) } : res, { open: 3, prev: prevRes && prevRes.data !== undefined ? { data: prevRes.data, ...(prevRes.extensions ? { extensions: prevRes.extensions } : {}) } : prevRes, errors: errorPaths })}</div>${resolverTiming(res.extensions)}`;
      } else response = `<div class="note">${bodyNote(e)}</div>`;
      body = `<div class="cols"><div>
          <div class="sect">${op.query ? "Query" : "Persisted query"}<span class="aside">${op.guessed ? "type guessed from the name" : ""}</span></div><div class="block">${query}</div>
          <div class="sect" style="border-top:1px solid var(--line)">Variables</div><div class="block">${vars}</div>
          ${op.extensions && !op.persisted ? `<div class="sect">Extensions</div><div class="block">${treeHtml(op.extensions, { open: 1 })}</div>` : ""}
        </div><div>
          <div class="sect">Response${prev ? `<span class="aside">changes since #${g.calls.indexOf(prev) + 1} marked</span>` : ""}</div>${response}
        </div></div>`;
    }
    const opPicker =
      ops.length > 1
        ? `<div class="strip"><span class="chip" style="background:none;padding-left:0">In this batch</span>${ops
            .map((o, j) => {
              const r = Array.isArray(e.json) ? e.json[j] : null;
              const bad = Array.isArray(r?.errors) && r.errors.length;
              return `<button class="chip${j === i ? " on" : ""}" data-op="${j}" style="${bad ? "color:var(--err)" : ""}">${j + 1} · ${esc(o.name)}</button>`;
            })
            .join("")}</div>`
        : "";
    return `${backHtml(wide) ? `<div style="padding:10px 16px 0">${backHtml(wide)}</div>` : ""}
      <div class="dhead"><span class="badge ${op.type === "subscription" ? "b-live" : "b-gql"}" style="height:22px;font-size:11.5px">${op.type}</span><h2>${esc(op.name)}</h2><span class="where">${esc(e.method)} ${esc(hostPath(e.url))}</span>${actionsHtml(e)}</div>
      <div class="strip">${status}${e.end ? `<span class="chip">${ms(e.end - e.start)}${e.timing ? ` · server ${ms((e.timing.receiveHeadersEnd - e.timing.sendEnd) / 1000)}` : ""}</span>` : ""}${e.size != null ? `<span class="chip">${bytes(e.size)}</span>` : ""}${client ? `<span class="chip">${esc(client)}${version ? " " + esc(version) : ""}</span>` : ""}${op.persisted ? `<span class="chip mono">persisted ${esc(op.persisted.slice(0, 10))}…</span>` : ""}${trace ? `<span class="chip mono" title="${esc(trace)}">trace ${esc(short(trace, 14))}</span>` : ""}${e.replay ? `<span class="chip">replayed from Circle</span>` : ""}</div>
      ${opPicker}${callsHtml(g, e)}
      ${tabsHtml([["main", "Operation"], ["headers", "Headers"], ["timing", "Timing"], ["raw", "Raw"]])}
      ${body}`;
  };

  const restDetail = (g, e, wide) => {
    const r = e.route;
    const prev = ui.diff ? previousCall(g, e) : null;
    let body;
    if (ui.tab === "headers") body = headersHtml(e);
    else if (ui.tab === "timing") body = timingHtml(e);
    else if (ui.tab === "raw") body = rawHtml(e);
    else {
      const important = Object.entries(e.reqHeaders || {}).filter(([k]) => IMPORTANT.test(k));
      const reqBody = e.postData != null ? (tryJson(e.postData) !== undefined ? treeHtml(tryJson(e.postData), { open: 3 }) : `<pre class="code">${esc(short(e.postData, 20000))}</pre>`) : "";
      const mime = e.mime || header(e.resHeaders, "content-type") || "";
      let response;
      if (e.bodyState === "done" && e.json !== undefined) {
        response = ui.respView === "raw" ? `<pre class="code">${esc(short(pretty(e.body), 400000))}</pre>` : treeHtml(e.json, { open: 2, prev: prev && e.body.length < DIFF_LIMIT ? prev.json : undefined, filter: ui.respFilter });
      } else if (e.bodyState === "done" && e.body != null) response = `<pre class="code" style="white-space:pre-wrap">${esc(short(e.body, 200000))}</pre>`;
      else if (e.bodyState === "done") response = `<div class="note" style="padding:0">Binary (${esc(mime)}), ${bytes(fromBase64(e.base64 || "").length)}</div>`;
      else response = `<div class="note" style="padding:0">${bodyNote(e)}</div>`;
      body = `<div class="cols"><div>
          ${r.params.length ? `<div class="sect">Path parameters<span class="aside mono">${templateHtml(r.template)}</span></div><div class="block">${kvHtml(r.params.map((p) => [":" + p.name, esc(p.value)]))}</div>` : ""}
          ${r.query.length ? `<div class="sect">Query</div><div class="block">${kvHtml(r.query.map(([k, v]) => [k, esc(v)]))}</div>` : ""}
          ${reqBody ? `<div class="sect">Body<span class="aside">${esc(header(e.reqHeaders, "content-type") || "")}</span></div><div class="block">${reqBody}</div>` : ""}
          <div class="sect">Headers that matter<span class="aside"><a href="#" data-act="headers">All ${Object.keys(e.reqHeaders || {}).length} request headers</a></span></div>
          <div class="block">${kvHtml(headerRows(Object.fromEntries(important), true), "None of note")}</div>
        </div><div>
          <div class="sect">Response${e.status ? ` · <span style="color:${e.status >= 400 ? "var(--err)" : "var(--ok)"};text-transform:none;letter-spacing:0;font-weight:600">${e.status} ${esc(e.statusText || "")}</span>` : ""}<span class="aside">${esc(mime)}${e.size != null ? " · " + bytes(e.size) : ""}</span></div>
          ${e.json !== undefined ? `<div style="display:flex;gap:8px;align-items:center;padding:2px 16px 6px;flex-wrap:wrap"><div class="seg" role="group" aria-label="View"><button class="${ui.respView === "tree" ? "on" : ""}" data-resp="tree">Tree</button><button class="${ui.respView === "raw" ? "on" : ""}" data-resp="raw">Raw</button></div>${g.calls.length > 1 ? `<label class="note" style="padding:0;display:flex;gap:6px;align-items:center"><input type="checkbox" data-act="diff" ${ui.diff ? "checked" : ""}>Show changes since ${prev ? "#" + (g.calls.indexOf(prev) + 1) : "the previous call"}</label>` : ""}</div>
          ${ui.respView === "tree" ? `<label class="filterbox"><input id="respfilter" aria-label="Filter the response" placeholder="Filter keys and values" value="${esc(ui.respFilter)}" spellcheck="false"></label>` : ""}` : ""}
          <div class="block">${response}</div>
          ${e.json !== undefined ? `<div class="note">Click a value to copy it.</div>` : ""}
        </div></div>`;
    }
    return `${backHtml(wide) ? `<div style="padding:10px 16px 0">${backHtml(wide)}</div>` : ""}
      <div class="dhead"><span class="badge ${METHOD_CLASS[e.method] || "b-plain"}" style="height:22px;font-size:11.5px">${esc(e.method)}</span><h2 class="mono">${pathHtml(e)}</h2>${actionsHtml(e)}</div>
      <div class="strip">${e.failed ? `<span class="chip err">${esc(e.failed)}</span>` : e.status ? `<span class="chip ${e.status >= 400 ? "err" : "ok"}">${e.status} ${esc(e.statusText || "")}</span>` : `<span class="chip">Waiting…</span>`}${e.end ? `<span class="chip">${ms(e.end - e.start)}</span>` : ""}${e.status === 304 || e.fromCache ? `<span class="chip">from cache</span>` : ""}<span class="chip mono">${esc(r.origin)}</span>${e.replay ? `<span class="chip">replayed from Circle</span>` : ""}${e.status >= 400 && restMessage(e) ? `<span class="chip err">${esc(short(restMessage(e), 120))}</span>` : ""}</div>
      ${callsHtml(g, e)}
      ${tabsHtml([["main", "Overview"], ["headers", "Headers"], ["timing", "Timing"], ["raw", "Raw"]])}
      ${body}`;
  };

  const assetDetail = (g, e, wide) => {
    let body;
    if (ui.tab === "timing") body = timingHtml(e);
    else body = headersHtml(e);
    return `${backHtml(wide) ? `<div style="padding:10px 16px 0">${backHtml(wide)}</div>` : ""}
      <div class="dhead">${badgeOf(e)}<h2 class="mono" style="font-size:13px">${esc(e.url)}</h2>${actionsHtml(e)}</div>
      <div class="strip">${e.failed ? `<span class="chip err">${esc(e.failed)}</span>` : e.status ? `<span class="chip ${e.status >= 400 ? "err" : "ok"}">${e.status} ${esc(e.statusText || "")}</span>` : ""}${e.mime ? `<span class="chip">${esc(e.mime)}</span>` : ""}${e.size != null ? `<span class="chip">${bytes(e.size)}</span>` : ""}${e.end ? `<span class="chip">${ms(e.end - e.start)}</span>` : ""}</div>
      ${tabsHtml([["main", "Headers"], ["timing", "Timing"]])}${body}`;
  };

  // ----- live connections ---------------------------------------------------

  const parseMsgQuery = (q) => {
    const f = { words: [], type: [], notType: [], id: null, op: null, dir: null, min: null };
    for (const word of q.trim().split(/\s+/).filter(Boolean)) {
      let m;
      if ((m = /^(-?)type:(.+)$/i.exec(word))) (m[1] ? f.notType : f.type).push(m[2].toLowerCase());
      else if ((m = /^id:(.+)$/i.exec(word))) f.id = m[1];
      else if ((m = /^op:(.+)$/i.exec(word))) f.op = m[1].toLowerCase();
      else if ((m = /^dir:(in|out|sent|received)$/i.exec(word))) f.dir = /in|received/i.test(m[1]) ? "in" : "out";
      else if ((m = /^size>(\d+)(kb|b)?$/i.exec(word))) f.min = +m[1] * (/kb/i.test(m[2] || "") ? 1024 : 1);
      else f.words.push(word.toLowerCase());
    }
    return f;
  };
  const msgVisible = (m, f) => {
    if (ui.ws.dir !== "both" && m.dir !== ui.ws.dir) return false;
    if (f.dir && m.dir !== f.dir) return false;
    const type = String(m.type).toLowerCase();
    if (f.type.length && !f.type.includes(type)) return false;
    if (f.notType.includes(type)) return false;
    if (f.id && m.id !== f.id) return false;
    if (f.op && !(m.op || "").toLowerCase().includes(f.op)) return false;
    if (f.min && m.size <= f.min) return false;
    if (f.words.length) {
      const hay = (m.text || m.preview || "").toLowerCase() + " " + type + " " + (m.op || "").toLowerCase();
      if (!f.words.every((w) => hay.includes(w))) return false;
    }
    return true;
  };

  const connName = (c) => PROTOCOL_NAMES[c.proto] || (c.kind === "sse" ? "Server-sent events" : "WebSocket");
  const connState = (c) => (c.error ? "bad" : c.closed ? "off" : "");

  const msgBadge = (m) => {
    const cls = m.error ? "b-err" : m.heartbeat ? "b-plain" : m.dir === "out" ? "b-post" : "b-live";
    return `<span class="badge ${cls} mono" style="font-weight:600">${esc(short(m.type, 18))}${m.id != null && m.id.length < 8 ? " #" + esc(m.id) : ""}</span>`;
  };

  const msgsHtml = (c) => {
    const f = parseMsgQuery(ui.ws.q);
    const all = ui.ws.paused ? c.frames.filter((m) => m.n <= ui.ws.paused) : c.frames;
    const shown = all.filter((m) => msgVisible(m, f));
    let html = "";
    let run = [];
    const flush = () => {
      if (!run.length) return;
      if (run.length === 1) html += msgRow(c, run[0]);
      else {
        const types = [...new Set(run.map((m) => m.type))].join("/");
        const gap = run.length > 1 ? (run[run.length - 1].t - run[0].t) / (run.length - 1) : 0;
        html += `<button class="msg hb" data-msg="${run[run.length - 1].n}"><span>⇅</span><span class="t">+${(run[0].t - c.start).toFixed(1)}</span><span class="badge b-plain mono">${esc(types)}</span><span class="pv">×${run.length} heartbeats${gap > 1 ? `, every ${Math.round(gap)} s` : ""}</span><span class="num">—</span></button>`;
      }
      run = [];
    };
    for (const m of shown.slice(-1500)) {
      if (m.heartbeat) run.push(m);
      else {
        flush();
        html += msgRow(c, m);
      }
    }
    flush();
    return { html, shown: shown.length, total: c.frames.length };
  };
  const msgRow = (c, m) =>
    `<button class="msg${ui.ws.msg === m.n ? " sel" : ""}" data-msg="${m.n}"><span class="${m.dir === "out" ? "up" : "down"}" aria-label="${m.dir === "out" ? "Sent" : "Received"}">${m.dir === "out" ? "↑" : "↓"}</span><span class="t">+${(m.t - c.start).toFixed(2)}</span>${msgBadge(m)}<span class="pv">${m.op ? `<b>${esc(m.op)}</b> ` : ""}<span class="mono">${esc(m.preview || "")}</span></span><span class="num">${bytes(m.size)}</span></button>`;

  const msgDetailHtml = (c, m, narrow) => {
    if (!m) {
      return `<div class="empty"><b>${esc(connName(c))}</b>${esc(c.url)}<br><br>${c.frames.length ? "Pick a message to see its payload and what changed since the last one like it." : "No messages yet."}</div>`;
    }
    const same = c.frames.filter((x) => x.key === m.key);
    const index = same.indexOf(m);
    const prev = index > 0 ? same[index - 1] : null;
    let body;
    const tab = ui.ws.tab;
    if (tab === "raw") body = `<div class="block"><pre class="code" style="white-space:pre-wrap">${m.binary ? esc(hexDump(m.binary)) : esc(short(pretty(m.text), 200000))}</pre></div>`;
    else if (tab === "handshake")
      body = `<div class="sect">Request</div><div class="block">${kvHtml(headerRows(c.reqHeaders, true), "Not seen")}</div><div class="sect">Response${c.status ? " · " + c.status : ""}</div><div class="block">${kvHtml(headerRows(c.resHeaders), "Not received")}</div>`;
    else if (tab === "changes") {
      body = prev
        ? `<div class="note" style="padding-top:10px">Compared with the previous <span class="mono">${esc(m.type)}${m.id ? " #" + esc(m.id) : ""}</span> at +${(prev.t - c.start).toFixed(2)} s</div><div class="block">${m.json !== undefined ? treeHtml(m.json, { open: 3, prev: prev.json }) : `<pre class="code"><span class="old">${esc(short(prev.text || "", 2000))}</span>\n${esc(short(m.text || "", 2000))}</pre>`}</div>`
        : `<div class="note" style="padding-top:12px">This is the first <span class="mono">${esc(m.type)}</span>${m.id ? " for #" + esc(m.id) : ""}, so there's nothing to compare it with.</div>`;
    } else body = `<div class="block" style="padding-top:10px">${m.json !== undefined ? treeHtml(m.json, { open: 3 }) : m.binary ? `<pre class="code">${esc(hexDump(m.binary))}</pre>` : `<pre class="code" style="white-space:pre-wrap">${esc(short(m.text, 200000))}</pre>`}</div>`;
    const title = m.op ? `${m.op} ${m.type === "next" || m.type === "data" ? "update" : m.type}` : m.type;
    return `${narrow ? `<div style="padding:10px 16px 0"><button class="back" data-act="ws-back">${ICON_BACK}Messages</button></div>` : ""}
      <div class="dhead">${msgBadge(m)}<h2>${esc(title)}</h2><span class="where">${m.dir === "out" ? "sent" : "received"} +${(m.t - c.start).toFixed(2)} s · ${bytes(m.size)}</span>
      <div class="acts"><button class="btn" data-act="copy-msg">Copy payload</button></div></div>
      <div class="strip">${same.length > 1 ? `<span class="chip">${ordinal(index + 1)} of ${same.length}${m.id ? " for #" + esc(m.id) : ""}</span>` : ""}${prev ? `<span class="chip">${ms(m.t - prev.t)} after the previous</span>` : ""}${m.eventId ? `<span class="chip mono">id ${esc(m.eventId)}</span>` : ""}${m.error ? `<span class="chip err">error</span>` : ""}</div>
      <div class="tabs" role="tablist">${[["payload", "Payload"], ["changes", "Changes"], ["raw", m.binary ? "Bytes" : "Raw"], ["handshake", c.kind === "sse" ? "Request" : "Handshake"]]
        .map(([id, label]) => `<button role="tab" aria-selected="${tab === id}" class="${tab === id ? "on" : ""}" data-wstab="${id}">${label}</button>`)
        .join("")}</div>${body}`;
  };
  const ordinal = (n) => n + (["th", "st", "nd", "rd"][((n % 100) - 20) % 10] || ["th", "st", "nd", "rd"][n % 100] || "th");
  const hexDump = (u8) => {
    let out = "";
    for (let i = 0; i < Math.min(u8.length, 4096); i += 16) {
      const row = u8.slice(i, i + 16);
      out += i.toString(16).padStart(6, "0") + "  " + Array.from(row, (b) => b.toString(16).padStart(2, "0")).join(" ").padEnd(48) + "  " + Array.from(row, (b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : ".")).join("") + "\n";
    }
    return out + (u8.length > 4096 ? `… ${u8.length - 4096} more bytes` : "");
  };

  const connsHtml = (conns, chips) =>
    conns
      .map((c) => {
        const sel = c.id === ui.ws.conn;
        if (chips) return `<button class="chip${sel ? " on" : ""}" data-conn="${esc(c.id)}"><span class="dot ${connState(c)}" style="display:inline-block;margin-right:6px"></span>${esc(connName(c))} · ${esc(hostPath(c.url).slice(0, 40))}</button>`;
        const age = ago(Math.max(0, (c.closed ? c.end || c.last || c.start : log.lastTime) - c.start));
        return `<div class="conn${sel ? " sel" : ""}" data-conn="${esc(c.id)}">
          <div class="l1"><span class="dot ${connState(c)}"></span><b>${esc(connName(c))}</b><span class="end">${c.closed ? "closed" : age}</span></div>
          <div class="u" title="${esc(c.url)}">${esc(c.url)}</div>
          <div class="c">${c.kind === "ws" ? `<span>↑ ${c.sent}</span>` : ""}<span>↓ ${c.recv}</span>${Object.keys(c.ops).length ? `<span style="color:var(--muted)">${Object.keys(c.ops).length} subscription${Object.keys(c.ops).length > 1 ? "s" : ""}</span>` : ""}</div>
          ${c.error ? `<div class="c" style="color:var(--err)">${esc(c.error)}</div>` : ""}
        </div>`;
      })
      .join("");

  // ----- edit and replay ----------------------------------------------------

  const openReplay = (e, run) => {
    const op = e.kind === "graphql" ? e.gql.ops[Math.min(ui.op, e.gql.ops.length - 1)] : null;
    const headers = Object.entries(sendableHeaders(e.reqHeaders))
      .map(([k, v]) => `${k}: ${v}`)
      .join("\n");
    ui.replay = {
      base: e,
      kind: e.kind,
      op,
      method: e.method,
      url: e.url,
      query: op ? (op.query ? gqlFormat(op.query) : "") : "",
      variables: op ? JSON.stringify(op.variables ?? {}, null, 2) : "",
      headers,
      body: e.kind === "rest" ? (e.postData != null ? pretty(e.postData) : "") : "",
      runs: [],
      shown: -1,
      compare: "diff",
      ask: false,
    };
    ui.view = "replay";
    ui.menu = false;
    layoutKey = "";
    render();
    if (run) send();
  };

  const changesData = (method, gqlType) => (gqlType ? gqlType === "mutation" : !["GET", "HEAD", "OPTIONS"].includes(method));

  const buildRequest = () => {
    const r = ui.replay;
    const headers = {};
    for (const line of r.headers.split("\n")) {
      const i = line.indexOf(":", line.startsWith(":") ? 1 : 0);
      if (i > 0) headers[line.slice(0, i).trim()] = line.slice(i + 1).trim();
    }
    if (r.kind === "graphql") {
      let variables;
      try {
        variables = r.variables.trim() ? JSON.parse(r.variables) : undefined;
      } catch (err) {
        throw new Error("Variables aren't valid JSON: " + err.message);
      }
      const name = gqlDefinitions(r.query)[0]?.name || r.op.operationName;
      const payload = { operationName: name || undefined, query: r.query || undefined, variables };
      if (!r.query && r.op.extensions) payload.extensions = r.op.extensions;
      if (r.method === "GET") {
        const u = new URL(r.url);
        for (const [k, v] of Object.entries(payload)) {
          if (v === undefined) u.searchParams.delete(k);
          else u.searchParams.set(k, typeof v === "string" ? v : JSON.stringify(v));
        }
        return { method: "GET", url: u.href, headers, body: null, type: gqlDefinitions(r.query)[0]?.type || r.op.type };
      }
      if (!Object.keys(headers).some((k) => /^content-type$/i.test(k))) headers["content-type"] = "application/json";
      return { method: r.method, url: r.url, headers, body: JSON.stringify(payload), type: gqlDefinitions(r.query)[0]?.type || r.op.type };
    }
    return { method: r.method, url: r.url, headers, body: ["GET", "HEAD"].includes(r.method) || !r.body.trim() ? null : r.body };
  };

  const runInTab = async (l, req) => {
    const tree = await l.send("Page.getFrameTree");
    const world = await l.send("Page.createIsolatedWorld", { frameId: tree.frameTree.frame.id, worldName: "Circle Network" });
    const init = { method: req.method, headers: sendableHeaders(req.headers) };
    if (req.body != null) init.body = req.body;
    // An isolated world gets the page's origin and cookies, but not the
    // page's own wrappers around fetch.
    const expression = `(async () => {
      const url = ${JSON.stringify(req.url)}, init = ${JSON.stringify(init)};
      const t0 = performance.now();
      let r;
      try { r = await fetch(url, { ...init, credentials: "include" }); }
      catch (e) { r = await fetch(url, { ...init, credentials: "same-origin" }); }
      const text = await r.text();
      return { status: r.status, statusText: r.statusText, headers: [...r.headers], text, ms: performance.now() - t0 };
    })()`;
    l.replaying.push({ url: req.url, method: req.method });
    const out = await l.send("Runtime.evaluate", { expression, contextId: world.executionContextId, awaitPromise: true, returnByValue: true });
    if (out.exceptionDetails) throw new Error(out.exceptionDetails.exception?.description?.split("\n")[0] || out.exceptionDetails.text || "The request failed");
    return out.result.value;
  };

  const send = async (force) => {
    const r = ui.replay;
    if (!r || r.sending) return;
    let req;
    try {
      req = buildRequest();
    } catch (err) {
      r.inputError = err.message;
      renderReplayResult();
      return;
    }
    r.inputError = "";
    let host = "";
    try {
      host = new URL(req.url).host;
    } catch {}
    if (!force && changesData(req.method, req.type) && !confirmed.has(host)) {
      r.ask = true;
      renderReplayResult();
      return;
    }
    if (force) confirmed.add(host);
    r.ask = false;
    r.sending = true;
    renderReplayResult();
    const run = { n: r.runs.length + 1, req, at: Date.now() };
    try {
      const res = await runInTab(log, req);
      run.res = res;
      run.json = tryJson(res.text);
    } catch (err) {
      run.error = err.message;
    }
    r.sending = false;
    r.runs.push(run);
    r.shown = r.runs.length - 1;
    renderReplayResult();
  };

  const replaySkeleton = () => {
    const r = ui.replay;
    const e = r.base;
    const title = r.kind === "graphql" ? `<span class="badge ${r.op.type === "mutation" ? "b-gql" : "b-gql"}">${r.op.type} ${esc(r.op.name)}</span>` : `<span class="badge ${METHOD_CLASS[e.method] || "b-plain"}">${esc(e.method)}</span><span class="mono" style="font-size:12px">${esc(e.route?.template || "")}</span>`;
    const left =
      r.kind === "graphql"
        ? `<div class="sect">Send to</div>
           <div class="urlrow"><input id="rp-url" aria-label="URL" value="${esc(r.url)}" spellcheck="false"></div>
           <div class="sect">Operation${r.query ? "" : '<span class="aside">persisted: add the query to send it in full</span>'}</div>
           <div class="editor"><pre aria-hidden="true" id="rp-hl"></pre><textarea id="rp-query" aria-label="Query" spellcheck="false" rows="${Math.min(22, Math.max(6, r.query.split("\n").length + 1))}">${esc(r.query)}</textarea></div>
           <div class="sect">Variables</div>
           <div class="editor"><textarea class="plain" id="rp-vars" aria-label="Variables" spellcheck="false" rows="${Math.min(16, Math.max(4, r.variables.split("\n").length + 1))}">${esc(r.variables)}</textarea></div>
           <div class="bad-input" id="rp-varerr"></div>`
        : `<div class="sect">Request</div>
           <div class="urlrow"><select id="rp-method" aria-label="Method">${["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].map((m) => `<option${m === r.method ? " selected" : ""}>${m}</option>`).join("")}</select><input id="rp-url" aria-label="URL" value="${esc(r.url)}" spellcheck="false"></div>
           <div class="sect">Body</div>
           <div class="editor"><textarea class="plain" id="rp-body" aria-label="Body" spellcheck="false" rows="${Math.min(18, Math.max(5, r.body.split("\n").length + 1))}">${esc(r.body)}</textarea></div>
           <div class="bad-input" id="rp-varerr"></div>`;
    return `<div class="pane detail" style="flex:1"><div class="scroll">
      <div class="dhead"><button class="back" data-act="replay-back">${ICON_BACK}Network</button><h2 style="font-size:15px">Edit &amp; replay</h2>${title}
        <div class="acts"><span class="chip ok" title="Sent from the page itself">Uses the tab's cookies and session</span><button class="btn primary" data-act="send">Send ${navigator.platform.startsWith("Mac") ? "⌘↵" : "Ctrl+↵"}</button></div></div>
      <div class="cols"><div>
        ${left}
        <div class="sect">Headers<span class="aside">one per line</span></div>
        <div class="editor"><textarea class="plain" id="rp-headers" aria-label="Headers" spellcheck="false" rows="${Math.min(10, Math.max(3, r.headers.split("\n").length + 1))}">${esc(r.headers)}</textarea></div>
        <div class="note">Cookies are added by the browser. Headers browsers don't allow scripts to set are left out.</div>
      </div><div id="rp-result"></div></div>
    </div></div>`;
  };

  const renderReplayResult = () => {
    const box = $("rp-result");
    const r = ui.replay;
    if (!box || !r) return;
    const err = $("rp-varerr");
    if (err) err.textContent = r.inputError || "";
    const run = r.runs[r.shown];
    let html = `<div class="sect">Result${run?.res ? `<span style="text-transform:none;letter-spacing:0;font-weight:600;color:${run.res.status >= 400 || gqlErrorCount(run.json) ? "var(--err)" : "var(--ok)"}">${run.res.status}${r.kind === "graphql" ? (gqlErrorCount(run.json) ? ` · ${gqlErrorCount(run.json)} error${gqlErrorCount(run.json) > 1 ? "s" : ""}` : " · no errors") : ""}</span><span style="text-transform:none;letter-spacing:0">${ms(run.res.ms / 1000)}</span>` : ""}
      <span class="aside"><span class="seg" role="group" aria-label="Compare"><button class="${r.compare === "response" ? "on" : ""}" data-compare="response">Response</button><button class="${r.compare === "diff" ? "on" : ""}" data-compare="diff">Compare with original</button></span></span></div>`;
    if (r.ask) {
      let host = "";
      try {
        host = new URL(buildRequest().url).host;
      } catch {}
      html += `<div class="warn"><span>This ${r.kind === "graphql" ? "is a mutation" : "request can change data"}: replaying it changes real data on <b>${esc(host)}</b>.</span><button class="btn" data-act="cancel-send">Cancel</button><button class="btn primary" data-act="send-anyway">Send anyway</button></div>`;
    }
    if (r.sending) html += `<div class="note" style="padding-top:8px"><span class="spin"></span> Sending from the tab…</div>`;
    if (!run && !r.sending && !r.ask) html += `<div class="note" style="padding-top:8px">Change anything on the left, then Send. The request goes out from the tab itself, with its cookies and session.</div>`;
    if (run?.error) html += `<div class="errcard"><div class="t"><b>${esc(run.error)}</b></div><div class="path" style="color:var(--muted)">CORS, a network error, or the tab navigated away.</div></div>`;
    if (run?.res) {
      const original = r.base.json;
      const now = run.json;
      if (r.kind === "graphql" && Array.isArray(now?.errors)) {
        html += now.errors.map((x) => `<div class="errcard"><div class="t"><b>${esc(x.message || "Error")}</b>${x.extensions?.code ? `<span class="code">${esc(x.extensions.code)}</span>` : ""}</div>${x.path ? `<div class="path">${esc(x.path.join(" › "))}</div>` : ""}</div>`).join("");
      }
      const view = (v, text, prev) => (v !== undefined ? treeHtml(v, { open: 3, prev }) : `<pre class="code" style="white-space:pre-wrap">${esc(short(text || "", 100000))}</pre>`);
      const originalOne = Array.isArray(original) && r.kind === "graphql" ? original[r.base.gql.ops.indexOf(r.op)] : original;
      if (r.compare === "diff" && originalOne !== undefined) {
        html += `<div class="sbs"><div><div class="cap">Original · from the page${r.base.status ? " · " + r.base.status : ""}</div>${treeHtml(originalOne, { open: 3 })}</div><div><div class="cap">Replay #${run.n}${now === undefined ? "" : " · changes marked"}</div>${view(now, run.res.text, now !== undefined ? originalOne : undefined)}</div></div>`;
      } else html += `<div class="block">${view(now, run.res.text)}</div>`;
      html += `<div class="sect">Response headers</div><div class="block">${kvHtml(run.res.headers.map(([k, v]) => [k, esc(v)]))}</div>`;
    }
    if (r.runs.length) {
      html += `<div class="sect" style="border-top:1px solid var(--line)">Replays</div><div style="padding:0 10px 8px">${r.runs
        .map((x, i) => {
          const n = gqlErrorCount(x.json);
          const outcome = x.error ? `<span style="color:var(--err)">failed</span>` : x.res.status >= 400 || n ? `<span style="color:var(--err)">${x.res.status}${n ? ` · ${n} error${n > 1 ? "s" : ""}` : ""}</span>` : `<span style="color:var(--ok)">${x.res.status}</span>`;
          return `<button class="hist${i === r.shown ? " on" : ""}" data-run="${i}"><span>#${x.n}</span><span class="mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:11.5px;color:var(--muted)">${esc(r.kind === "graphql" ? loose(tryJson(x.req.body || "")?.variables ?? new URL(x.req.url).searchParams.get("variables") ?? "", 80) : x.req.method + " " + hostPath(x.req.url))}</span>${outcome}<span class="num">${x.res ? ms(x.res.ms / 1000) : ""}</span></button>`;
        })
        .reverse()
        .join("")}</div>
        <div style="display:flex;gap:14px;padding:0 16px 14px;font-size:12px"><a href="#" data-act="copy-run-curl">Copy as cURL</a><a href="#" data-act="copy-run-fetch">Copy as fetch</a></div>`;
    }
    box.innerHTML = html;
  };
  const gqlErrorCount = (j) => (Array.isArray(j?.errors) ? j.errors.length : 0);

  const highlightEditor = () => {
    const area = $("rp-query");
    const hl = $("rp-hl");
    if (!area || !hl) return;
    hl.innerHTML = gqlHighlight(area.value) + "\n";
    hl.scrollTop = area.scrollTop;
    hl.scrollLeft = area.scrollLeft;
  };

  // ----- drawing --------------------------------------------------------------

  let layoutKey = "";
  let detailKey = "";
  let scheduled = false;
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    setTimeout(() => {
      scheduled = false;
      render();
    }, 150);
  };

  const counts = (l) => {
    const n = { all: 0, graphql: 0, rest: 0, live: 0, asset: 0 };
    for (const e of l.entries) {
      n[e.kind]++;
      if (e.kind !== "asset") n.all++;
    }
    n.all += ui.showAssets ? n.asset : 0;
    return n;
  };

  const renderChrome = () => {
    document.documentElement.dataset.theme = state?.dark === false ? "light" : "dark";
    document.body.className = state?.dock === "bottom" ? "bottom" : "right";
    $("tab").textContent = state?.title || "";
    const rec = $("rec");
    const on = !!log?.open;
    rec.className = "rec" + (on ? " on" : "");
    rec.lastElementChild.textContent = on ? "Recording" : state?.live ? "Connecting…" : "Off";
    $("preserve").classList.toggle("on", ui.preserve);
    $("preserve").setAttribute("aria-pressed", ui.preserve);
    const n = log ? counts(log) : { all: 0, graphql: 0, rest: 0, live: 0, asset: 0 };
    const kinds = [
      ["all", "All"],
      ["graphql", "GraphQL"],
      ["rest", "REST"],
      ["live", "Live"],
      ["asset", "Page & assets"],
    ];
    circle.html(
      $("kinds"),
      kinds.map(([k, label]) => `<button class="kind ${k}${ui.kind === k ? " on" : ""}" data-kind="${k}" aria-pressed="${ui.kind === k}">${label} ${n[k]}</button>`).join(""),
    );
    if (!log) return circle.html($("foot"), "");
    const total = log.entries.reduce((s, e) => s + (e.size || 0), 0);
    const problems = log.entries.filter(problem);
    const gqlErrs = problems.filter((e) => e.gqlErrors > 0).length;
    const http = problems.length - gqlErrs;
    const starts = log.entries.filter((e) => e.kind !== "live").map((e) => e.start);
    const ends = log.entries.filter((e) => e.end && e.kind !== "live").map((e) => e.end);
    const span = starts.length && ends.length ? Math.max(...ends) - Math.min(...starts) : 0;
    circle.html(
      $("foot"),
      `<span>${log.entries.length} request${log.entries.length === 1 ? "" : "s"}</span><span>${bytes(total)}</span>${span ? `<span>${ms(span)}</span>` : ""}${problems.length ? `<button class="bad" data-act="problems">${problems.length} problem${problems.length > 1 ? "s" : ""}${gqlErrs ? ` · ${gqlErrs} with GraphQL errors` : ""}${http && gqlErrs ? ` · ${http} HTTP` : ""}</button>` : ""}<span class="hint">${navigator.platform.startsWith("Mac") ? "⌥⌘E" : "Ctrl+Shift+E"} closes · DevTools stay on ${navigator.platform.startsWith("Mac") ? "⌥⌘I" : "F12"}</span>`,
    );
  };

  const setLayout = (key, html) => {
    if (layoutKey === key) return false;
    layoutKey = key;
    detailKey = "";
    main.innerHTML = html;
    return true;
  };

  const emptyState = (title, text, button) =>
    setLayout("empty:" + title + text, `<div class="empty"><b>${title}</b>${text}${button || ""}</div>`);

  const render = () => {
    renderChrome();
    if (!state?.tab) return emptyState("No page", "Open a page to see what it sends and receives.");
    if (!state.live) return emptyState("This tab isn't loaded", "Its requests show up once it loads.");
    if (!log) return;
    if (ui.view === "replay" && ui.replay) {
      if (setLayout("replay:" + ui.replay.base.key, replaySkeleton())) {
        highlightEditor();
        renderReplayResult();
      }
      return;
    }
    const wide = main.clientWidth >= 860;
    main.classList.toggle("narrow", main.clientWidth < 560);
    if (ui.kind === "live") return renderLive(wide);
    const groups = groupsFor(log);
    if (!log.entries.length) {
      return emptyState(
        log.open ? "Listening" : "Connecting…",
        log.open ? "Requests the page makes from now on show up here. Reload to see everything from the start." : "Connecting to the tab.",
        log.open ? `<br><button class="btn" data-act="reload">Reload page</button>` : "",
      );
    }
    const sel = selected();
    if (ui.sel && !sel) {
      ui.sel = null;
      ui.call = null;
    }
    const mode = sel ? (wide ? "split" : "detail") : "list";
    setLayout(
      "list:" + mode,
      mode === "list"
        ? `<div class="pane" style="flex:1"><div class="scroll" id="list"></div></div>`
        : mode === "split"
          ? `<div class="pane side"><div class="scroll" id="list"></div></div><div class="pane detail"><div class="scroll" id="detail"></div></div>`
          : `<div class="pane detail" style="flex:1"><div class="scroll" id="detail"></div></div>`,
    );
    const list = $("list");
    if (list) {
      const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
      let html = mode === "list" ? listHtml(groups) : sideHtml(groups);
      const hidden = ui.kind === "all" && !ui.showAssets ? log.entries.filter((e) => e.kind === "asset").length : 0;
      if (hidden) html += `<button class="more" data-act="assets">▸ ${hidden} page load${hidden > 1 ? "s" : ""}, scripts, styles and images hidden · show</button>`;
      else if (ui.kind === "all" && ui.showAssets) html += `<button class="more" data-act="assets">▾ Hide page loads, scripts, styles and images</button>`;
      if (!groups.length) html += `<div class="note" style="padding:16px">${ui.q ? "Nothing matches the filter." : ui.kind === "graphql" ? "No GraphQL yet. Operations show up here as the page sends them." : ui.kind === "rest" ? "No API calls yet." : "Nothing yet."}</div>`;
      circle.html(list, html);
      if (atBottom && mode === "list") list.scrollTop = list.scrollHeight;
    }
    const detail = $("detail");
    if (detail && sel) {
      const key = [sel.g.key, sel.e.key, sel.e.v, sel.g.calls.length, ui.tab, ui.op, ui.respView, ui.respFilter, ui.diff, ui.jwt, ui.menu, wide].join("|");
      if (key !== detailKey) {
        const focusFilter = document.activeElement?.id === "respfilter";
        const caret = focusFilter ? document.activeElement.selectionStart : 0;
        const keepScroll = detailKey.split("|")[1] === sel.e.key ? detail.scrollTop : 0;
        detailKey = key;
        detail.innerHTML = sel.e.kind === "graphql" ? graphqlDetail(sel.g, sel.e, wide) : sel.e.kind === "rest" ? restDetail(sel.g, sel.e, wide) : assetDetail(sel.g, sel.e, wide);
        detail.scrollTop = keepScroll;
        if (focusFilter) {
          const f = $("respfilter");
          f?.focus();
          f?.setSelectionRange(caret, caret);
        }
      }
    }
  };

  const renderLive = (wide) => {
    const conns = log.conns;
    if (!conns.length) {
      return emptyState("No live connections", "WebSockets and server-sent events the page opens show up here, with their messages.");
    }
    if (!conns.some((c) => c.id === ui.ws.conn)) {
      ui.ws.conn = (conns.find((c) => !c.closed) || conns[conns.length - 1]).id;
      ui.ws.msg = null;
    }
    const c = log.connById.get(ui.ws.conn);
    const msg = c.frames.find((m) => m.n === ui.ws.msg) || null;
    const mode = wide ? "wide" : msg ? "narrow-detail" : "narrow";
    const toolbar = `<div class="bar" style="padding:8px 12px">
        <label class="search"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/></svg><input id="wsq" aria-label="Filter messages" placeholder="type:next -type:ping id:3 op:Name size>1kb" spellcheck="false" class="mono" style="font-size:11.5px" value="${esc(ui.ws.q)}"></label>
        <div class="seg" role="group" aria-label="Direction" id="wsdir"></div>
        <button class="btn" id="wspause"></button>
      </div>`;
    const msgs = `<div class="pane msgs">${toolbar}<div class="scroll" id="wslist"></div><div class="note" id="wsfoot" style="padding:7px 12px;border-top:1px solid var(--line);display:flex;gap:12px"></div></div>`;
    setLayout(
      "live:" + mode,
      mode === "wide"
        ? `<div class="pane conns"><div class="sect">Connections</div><div class="scroll" id="wsconns"></div></div>${msgs}<div class="pane msgdetail"><div class="scroll" id="wsdetail"></div></div>`
        : mode === "narrow"
          ? `<div class="pane" style="flex:1"><div class="connchips" id="wsconns"></div>${msgs.replace('class="pane msgs"', 'class="pane msgs" style="flex:1;border:0"')}</div>`
          : `<div class="pane msgdetail" style="flex:1"><div class="scroll" id="wsdetail"></div></div>`,
    );
    const connsEl = $("wsconns");
    if (connsEl) circle.html(connsEl, connsHtml(conns, mode !== "wide"));
    const listEl = $("wslist");
    if (listEl) {
      const atBottom = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight < 40;
      const out = msgsHtml(c);
      circle.html(listEl, out.html || `<div class="note" style="padding:16px">${c.frames.length ? "No messages match." : "No messages yet."}</div>`);
      if (atBottom && !ui.ws.paused) listEl.scrollTop = listEl.scrollHeight;
      circle.html(
        $("wsdir"),
        [["both", "Both"], ["out", "↑ Sent"], ["in", "↓ Received"]].map(([d, label]) => `<button class="${ui.ws.dir === d ? "on" : ""}" data-dir="${d}">${label}</button>`).join(""),
      );
      $("wspause").textContent = ui.ws.paused ? `Resume${c.frames.length - ui.ws.paused > 0 ? ` (${c.frames.length - ui.ws.paused} new)` : ""}` : "Pause";
      circle.html(
        $("wsfoot"),
        `${ui.ws.paused ? "<span>Paused</span>" : `<span style="color:var(--ok)">● Following</span>`}<span>${out.shown} of ${out.total} shown</span><span style="margin-left:auto">${esc(connName(c))}${c.subprotocol ? " · " + esc(c.subprotocol) : ""}</span>`,
      );
    }
    const detail = $("wsdetail");
    if (detail) {
      const key = [c.id, msg?.n, ui.ws.tab, mode, msg ? 0 : c.frames.length].join("|");
      if (key !== detailKey) {
        detailKey = key;
        detail.innerHTML = msgDetailHtml(c, msg, mode !== "wide");
      }
    }
  };

  // ----- input ----------------------------------------------------------------

  const selectGroup = (key, op) => {
    if (ui.sel !== key) {
      ui.call = null;
      ui.respFilter = "";
      ui.jwt = false;
    }
    ui.sel = key;
    ui.op = op ?? 0;
    ui.menu = false;
    render();
  };

  document.addEventListener("click", (event) => {
    const t = event.target;
    const copyEl = t.closest("[data-copy]");
    if (copyEl && copyEl.closest(".tree")) {
      event.preventDefault();
      copy(copyEl.dataset.copy);
      return;
    }
    const el = t.closest("[data-kind],[data-group],[data-tab],[data-act],[data-call],[data-op],[data-resp],[data-copy-as],[data-conn],[data-msg],[data-dir],[data-wstab],[data-compare],[data-run]");
    if (ui.menu && !t.closest(".menu") && !t.closest('[data-act="menu"]')) {
      ui.menu = false;
      render();
    }
    if (!el) return;
    const d = el.dataset;
    if (d.kind) {
      ui.kind = d.kind;
      ui.sel = null;
      ui.call = null;
      ui.view = "list";
      render();
    } else if (d.group) selectGroup(d.group, d.op !== undefined ? +d.op : 0);
    else if (d.tab) {
      ui.tab = d.tab;
      render();
    } else if (d.call) {
      ui.call = d.call;
      render();
    } else if (d.op !== undefined) {
      ui.op = +d.op;
      render();
    } else if (d.resp) {
      ui.respView = d.resp;
      render();
    } else if (d.conn) {
      ui.ws.conn = d.conn;
      ui.ws.msg = null;
      ui.ws.paused = 0;
      render();
    } else if (d.msg) {
      ui.ws.msg = +d.msg;
      render();
    } else if (d.dir) {
      ui.ws.dir = d.dir;
      render();
    } else if (d.wstab) {
      ui.ws.tab = d.wstab;
      render();
    } else if (d.compare) {
      ui.replay.compare = d.compare;
      renderReplayResult();
    } else if (d.run) {
      ui.replay.shown = +d.run;
      renderReplayResult();
    } else if (d.copyAs) {
      const sel = selected();
      if (!sel) return;
      const e = sel.e;
      const op = e.gql?.ops[Math.min(ui.op, e.gql.ops.length - 1)];
      const text = {
        curl: () => curlOf(requestOf(e)),
        fetch: () => fetchOf(requestOf(e)),
        url: () => e.url,
        query: () => (op?.query ? gqlFormat(op.query) : op?.persisted || ""),
        variables: () => JSON.stringify(op?.variables ?? {}, null, 2),
        response: () => pretty(bodyText(e)),
      }[d.copyAs]();
      ui.menu = false;
      copy(text, d.copyAs === "curl" ? "Copied as cURL" : d.copyAs === "fetch" ? "Copied as fetch" : "Copied");
      render();
    } else if (d.act) {
      if (el.tagName === "A") event.preventDefault();
      act(d.act, el);
    }
  });

  const act = (name, el) => {
    const sel = selected();
    switch (name) {
      case "back":
        ui.sel = null;
        ui.call = null;
        render();
        break;
      case "assets":
        ui.showAssets = !ui.showAssets;
        render();
        break;
      case "menu":
        ui.menu = !ui.menu;
        render();
        break;
      case "jwt":
        ui.jwt = !ui.jwt;
        render();
        break;
      case "diff":
        ui.diff = el.checked;
        render();
        break;
      case "headers":
        ui.tab = "headers";
        render();
        break;
      case "problems":
        ui.kind = "all";
        ui.q = "has:errors";
        $("q").value = ui.q;
        ui.sel = null;
        render();
        break;
      case "reload":
        log?.send("Page.reload", {}).catch(() => {});
        break;
      case "replay":
        if (sel) openReplay(sel.e, true);
        break;
      case "edit":
        if (sel) openReplay(sel.e, false);
        break;
      case "replay-back":
        ui.view = "list";
        ui.replay = null;
        render();
        break;
      case "send":
        send(false);
        break;
      case "send-anyway":
        send(true);
        break;
      case "cancel-send":
        ui.replay.ask = false;
        renderReplayResult();
        break;
      case "copy-run-curl":
      case "copy-run-fetch": {
        const run = ui.replay?.runs[ui.replay.shown];
        if (run) copy(name === "copy-run-curl" ? curlOf(run.req) : fetchOf(run.req), "Copied");
        break;
      }
      case "ws-back":
        ui.ws.msg = null;
        render();
        break;
      case "copy-msg": {
        const c = log?.connById.get(ui.ws.conn);
        const m = c?.frames.find((x) => x.n === ui.ws.msg);
        if (m) copy(m.json !== undefined ? JSON.stringify(m.json, null, 2) : m.text || hexDump(m.binary || new Uint8Array()));
        break;
      }
    }
  };

  document.addEventListener("input", (event) => {
    const t = event.target;
    if (t.id === "q") {
      ui.q = t.value;
      render();
    } else if (t.id === "respfilter") {
      ui.respFilter = t.value;
      render();
    } else if (t.id === "wsq") {
      ui.ws.q = t.value;
      render();
    } else if (ui.replay && t.id?.startsWith("rp-")) {
      const r = ui.replay;
      if (t.id === "rp-query") {
        r.query = t.value;
        highlightEditor();
      } else if (t.id === "rp-vars") {
        r.variables = t.value;
        let message = "";
        try {
          if (t.value.trim()) JSON.parse(t.value);
        } catch (err) {
          message = "Not valid JSON yet: " + err.message;
        }
        $("rp-varerr").textContent = message;
      } else if (t.id === "rp-headers") r.headers = t.value;
      else if (t.id === "rp-url") r.url = t.value;
      else if (t.id === "rp-body") r.body = t.value;
    }
  });
  document.addEventListener("change", (event) => {
    if (event.target.id === "rp-method" && ui.replay) ui.replay.method = event.target.value;
  });
  document.addEventListener(
    "scroll",
    (event) => {
      if (event.target.id === "rp-query") highlightEditor();
    },
    true,
  );

  document.addEventListener("keydown", (event) => {
    const mod = event.metaKey || event.ctrlKey;
    if (mod && event.key === "Enter" && ui.view === "replay") {
      event.preventDefault();
      send(false);
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || "");
    if (event.key === "Tab" && event.target.tagName === "TEXTAREA") {
      event.preventDefault();
      document.execCommand("insertText", false, "  ");
      return;
    }
    if (typing) return;
    if (event.key === "Escape") {
      if (ui.menu) ui.menu = false;
      else if (ui.view === "replay") {
        ui.view = "list";
        ui.replay = null;
      } else if (ui.kind === "live" && ui.ws.msg) ui.ws.msg = null;
      else ui.sel = null;
      render();
    } else if ((event.key === "ArrowDown" || event.key === "ArrowUp") && log && ui.view === "list") {
      event.preventDefault();
      if (ui.kind === "live") {
        const c = log.connById.get(ui.ws.conn);
        if (!c) return;
        const f = parseMsgQuery(ui.ws.q);
        const shown = c.frames.filter((m) => msgVisible(m, f));
        const i = shown.findIndex((m) => m.n === ui.ws.msg);
        const next = shown[Math.max(0, Math.min(shown.length - 1, i < 0 ? shown.length - 1 : i + (event.key === "ArrowDown" ? 1 : -1)))];
        if (next) ui.ws.msg = next.n;
      } else {
        const groups = groupsFor(log);
        const i = groups.findIndex((g) => g.key === ui.sel);
        const next = groups[Math.max(0, Math.min(groups.length - 1, i < 0 ? groups.length - 1 : i + (event.key === "ArrowDown" ? 1 : -1)))];
        if (next) selectGroup(next.key);
        document.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
        return;
      }
      render();
      document.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
    } else if (event.key === "/" ) {
      event.preventDefault();
      (ui.kind === "live" ? $("wsq") : $("q"))?.focus();
    }
  });

  document.addEventListener("click", (event) => {
    if (event.target.closest("#wspause")) {
      const c = log?.connById.get(ui.ws.conn);
      ui.ws.paused = ui.ws.paused ? 0 : c?.n || 0;
      render();
    }
  });

  $("preserve").addEventListener("click", () => {
    ui.preserve = !ui.preserve;
    render();
  });
  $("clear").addEventListener("click", () => {
    if (!log) return;
    log.clear();
    ui.sel = null;
    ui.call = null;
    ui.ws.msg = null;
    ui.view = "list";
    render();
  });
  $("devtools").addEventListener("click", () => circle.action("dev_tools"));
  $("close").addEventListener("click", () => circle.action("network"));

  // Dragging the inner edge resizes the tool.
  const grip = $("grip");
  let drag = null;
  grip.addEventListener("pointerdown", (event) => {
    drag = [event.screenX, event.screenY];
    grip.setPointerCapture(event.pointerId);
    event.preventDefault();
    circle.send({ cmd: "network_grip", phase: "start" });
  });
  grip.addEventListener("pointermove", (event) => {
    if (drag) circle.send({ cmd: "network_grip", phase: "move", dx: Math.round(event.screenX - drag[0]), dy: Math.round(event.screenY - drag[1]) });
  });
  const endDrag = () => {
    if (drag) circle.send({ cmd: "network_grip", phase: "end" });
    drag = null;
  };
  grip.addEventListener("pointerup", endDrag);
  grip.addEventListener("pointercancel", endDrag);

  new ResizeObserver(() => schedule()).observe(main);

  circle.render = (s) => {
    const changedTab = state?.tab !== s.tab;
    state = s;
    if (s.tab != null) {
      if (!logs.has(s.tab)) logs.set(s.tab, new Log(s.tab));
      log = logs.get(s.tab);
      if (s.live) log.connect();
    } else log = null;
    if (changedTab) {
      ui.sel = null;
      ui.call = null;
      ui.view = "list";
      ui.replay = null;
      ui.ws.conn = null;
      ui.ws.msg = null;
      layoutKey = "";
    }
    render();
  };

  // What the tabs send, relayed by Circle.
  circle.cdp = (tab, messages) => {
    const l = logs.get(tab);
    if (l) for (const m of messages) l.receive(m);
  };
  circle.cdpState = (tab, open) => logs.get(tab)?.connected(open);

  render();
  circle.send({ cmd: "ready", from: "network" });
})();
