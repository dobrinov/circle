// The command bar: Spotlight (⌘K), new tab (⌘T), edit address (⌘L), and
// the find bar (⌘F). All but the find bar sit over a blurred snapshot of the
// page (circle.backdrop), with the best match shown large.
(() => {
  const $ = (id) => document.getElementById(id);
  const input = $("q");
  const list = $("results");

  const GLYPHS = {
    search: '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>',
    go: '<svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg>',
    cmd: '<svg viewBox="0 0 24 24"><path d="M8 5.5a2.5 2.5 0 1 0-2.5 2.5h13A2.5 2.5 0 1 0 16 5.5v13a2.5 2.5 0 1 0 2.5-2.5h-13A2.5 2.5 0 1 0 8 18.5z"/></svg>',
    history: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8"/><path d="M12 7.5V12l3 2"/></svg>',
    closed: '<svg viewBox="0 0 24 24"><path d="M4 12a8 8 0 1 0 2.3-5.7"/><path d="M4 4v4h4"/></svg>',
  };

  let state = null;
  let mode = null;
  let items = [];
  // History matches for the current query, fetched as the user types.
  let historyHits = { query: null, visits: [] };
  let selected = 0;
  let openedAt = 0;
  let lastHeight = 0;
  let words = [];

  const mac = () => state?.platform === "macos";
  const looksLikeUrl = (text) =>
    /^[a-z][a-z0-9+.-]*:/i.test(text) || (!/\s/.test(text) && /^[^/]+\.[a-z]{2,}(?::\d+)?(\/.*)?$/i.test(text)) || /^localhost(:\d+)?/.test(text);

  const COMMANDS = [
    ["New Space", () => circle.send({ cmd: "new_space" })],
    ["Toggle Sidebar", () => circle.action("toggle_sidebar")],
    ["Split View", () => circle.action("toggle_split")],
    ["Split View: Switch Direction (Side by Side / Stacked)", () => circle.action("toggle_split_direction")],
    ["Pin or Unpin Tab", () => circle.action("toggle_pin")],
    ["Copy URL", () => circle.action("copy_url")],
    ["Close Tab", () => circle.action("close_tab")],
    ["Reopen Closed Tab", () => circle.action("reopen_closed")],
    ["Clear Today's Tabs", () => circle.action("clear_today")],
    ["Find in Page", () => circle.action({ open_palette: "find" })],
    ["Reload", () => circle.action("reload")],
    ["Hard Reload", () => circle.action("reload_hard")],
    ["Zoom In", () => circle.action({ zoom: 1 })],
    ["Zoom Out", () => circle.action({ zoom: -1 })],
    ["Actual Size", () => circle.action({ zoom: 0 })],
    ["Developer Tools", () => circle.action("dev_tools")],
    ["View Page Source", () => circle.action("view_source")],
    [() => (state?.recording ? "Stop Recording" : "Record This Tab…"), () => circle.action(state?.recording ? "stop_recording" : { record: "video" })],
    ["Record This Tab as a GIF…", () => circle.action({ record: "gif" })],
    [() => (state?.spaces?.some((s) => s.private) ? "Go to Fap Mode" : "Fap Mode (private browsing)"), () => circle.action("private_space")],
    [() => (state?.spaces?.some((s) => s.private) ? "End Fap Mode" : ""), () => circle.action("end_private")],
    ["Settings", () => circle.action("show_settings")],
    ["What's New in Circle", () => circle.action("show_whats_new")],
    [() => (state?.bug_report ? "Finish Bug Report" : "Report a Bug"), () => (state?.bug_report ? circle.send({ cmd: "bug_report_stop" }) : circle.action("report_bug"))],
    ["History", () => circle.action("show_history")],
    ["Appearance: Dark", () => circle.action({ appearance: "dark" })],
    ["Appearance: Light", () => circle.action({ appearance: "light" })],
    ["Appearance: Match System", () => circle.action({ appearance: "system" })],
    ["Clear Browsing Data…", () => circle.action("clear_browsing_data")],
    ["Developer Tools: Dock to Right", () => circle.action({ dock_dev_tools: "right" })],
    ["Developer Tools: Dock to Bottom", () => circle.action({ dock_dev_tools: "bottom" })],
    ["Developer Tools: Open in Separate Window", () => circle.action({ dock_dev_tools: "window" })],
    [() => (state?.agent?.enabled ? "Turn Off Agent Control" : "Allow Agent Control"), () => circle.action("toggle_agent_access")],
    ["Next Space", () => circle.action("next_space")],
    ["Previous Space", () => circle.action("prev_space")],
    ["Toggle Full Screen", () => circle.action("fullscreen")],
    ["Quit Circle", () => circle.action("quit")],
  ];

  circle.render = (next) => {
    state = next;
    circle.setHue(state.spaces[state.active_space].hue);
    if (mode === "find") updateCount();
  };

  circle.open = (opts) => {
    mode = opts.mode;
    openedAt = Date.now();
    const backdrop = !!opts.backdrop;
    document.body.className = backdrop ? "backdrop" : mode === "find" ? "find" : "";
    document.body.classList.toggle("sharp", backdrop && opts.blur === false);
    document.body.classList.toggle("settings", mode === "settings");
    document.body.classList.toggle("whatsnew", mode === "whats_new");
    if (backdrop) {
      document.documentElement.style.setProperty("--frame", opts.frame);
      document.documentElement.style.setProperty("--panel-top", `${opts.top}px`);
      if (!opts.keep_backdrop) $("backdrop").innerHTML = "";
      $("backdrop").classList.toggle("splash", !!opts.splash);
    }
    input.placeholder = {
      find: "Find in page",
      spotlight: "Search tabs, history or the web",
      edit: "Enter an address or search",
    }[mode] || "Search or enter an address";
    if (mode === "settings") {
      openSettings();
      return;
    }
    if (mode === "whats_new") {
      $("whatsnew-version").textContent = `Circle ${opts.version}`;
      $("notes").innerHTML = renderNotes(opts.notes || "");
      $("notes").scrollTop = 0;
      $("got-it").focus();
      return;
    }
    input.value = opts.text || "";
    input.focus();
    input.select();
    selected = 0;
    refresh();
    fetchHistory();
    if (mode === "find" && input.value) circle.send({ cmd: "find", text: input.value });
  };

  // The blurred snapshot behind the panel: one image per visible page,
  // positioned where the page is.
  circle.backdrop = (images) => {
    const box = $("backdrop");
    box.innerHTML = "";
    for (const shot of images) {
      const img = new Image();
      img.alt = "";
      Object.assign(img.style, { left: `${shot.x}px`, top: `${shot.y}px`, width: `${shot.w}px`, height: `${shot.h}px` });
      img.onload = () => img.classList.add("ready");
      img.src = shot.src;
      box.append(img);
    }
  };

  // The CHANGELOG's Markdown, as much of it as release notes use: version
  // headings, section headings, bullets (with wrapped lines), `code`,
  // *emphasis* and **bold**. Several versions get their names as headings.
  function renderNotes(markdown) {
    const inline = (text) =>
      circle.esc(text)
        .replace(/`([^`]+)`/g, "<code>$1</code>")
        .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
        .replace(/\*([^*]+)\*/g, "<em>$1</em>");
    const several = (markdown.match(/^## \[/gm) || []).length > 1;
    const out = [];
    let list = null;
    let item = null;
    const flush = () => {
      if (item !== null) list.push(`<li>${inline(item)}</li>`);
      item = null;
    };
    const endList = () => {
      flush();
      if (list) out.push(`<ul>${list.join("")}</ul>`);
      list = null;
    };
    for (const line of markdown.split("\n")) {
      const version = line.match(/^## \[([^\]]+)\]/);
      if (version) {
        endList();
        if (several) out.push(`<h2>Circle ${circle.esc(version[1])}</h2>`);
      } else if (line.startsWith("### ")) {
        endList();
        out.push(`<h3>${inline(line.slice(4))}</h3>`);
      } else if (/^[-*] /.test(line)) {
        flush();
        list ||= [];
        item = line.slice(2);
      } else if (line.trim() && item !== null && /^\s/.test(line)) {
        item += ` ${line.trim()}`;
      } else if (line.trim()) {
        endList();
        out.push(`<p>${inline(line)}</p>`);
      }
    }
    endList();
    return out.join("");
  }
  $("got-it").onclick = () => close();
  $("release-notes").onclick = () => {
    close();
    circle.send({ cmd: "navigate", input: "https://github.com/dobrinov/circle/releases", new_tab: true });
  };
  document.addEventListener("keydown", (event) => {
    if (mode === "whats_new" && event.key === "Escape") {
      event.preventDefault();
      close();
    }
  });

  // Settings load once, in a frame, and stay loaded between openings.
  let settingsFrame = null;
  function openSettings() {
    if (!settingsFrame) {
      settingsFrame = document.createElement("iframe");
      settingsFrame.title = "Settings";
      settingsFrame.src = "settings.html?panel";
      $("settings").append(settingsFrame);
    }
    settingsFrame.focus();
    const find = settingsFrame.contentDocument?.getElementById("find-input");
    if (find) find.focus();
    else settingsFrame.addEventListener("load", () => settingsFrame.contentDocument?.getElementById("find-input")?.focus(), { once: true });
  }
  circle.settings = (s) => {
    settingsFrame?.contentWindow?.circle?.render?.(s);
  };

  function close() {
    mode = null;
    circle.send({ cmd: "close_palette" });
  }

  function updateCount() {
    const { count, active } = state?.palette.find || {};
    let label = "";
    if (mode === "find" && input.value) {
      // CEF reports no active match until the user steps through results.
      label = !count ? "No matches" : active ? `${active}/${count}` : `${count} matches`;
    }
    $("count").textContent = label;
  }

  // ----- results ---------------------------------------------------------------

  function buildItems(text) {
    const query = text.trim();
    const out = [];
    if (!state) return out;
    const newTab = mode !== "edit";
    const spotlight = mode === "spotlight";

    const spaceTabs = (s) => [...s.folders.flatMap((f) => f.tabs), ...s.pinned, ...s.tabs];
    const spaceOf = new Map(state.spaces.flatMap((s) => spaceTabs(s).map((t) => [t.id, s])));
    // The current space's tabs first, then favorites, then other spaces.
    const current = state.spaces[state.active_space];
    // Private and normal tabs never show up in each other's Spotlight.
    const others = state.spaces.filter((s) => s !== current && !!s.private === !!current.private);
    const favorites = current.private ? [] : state.favorites;
    const tabs = [...spaceTabs(current), ...favorites, ...others.flatMap(spaceTabs)].filter(
      (t) => t.id !== state.active,
    );
    const tabItem = (t, kind) => {
      const space = spaceOf.get(t.id);
      return {
        kind,
        icon: circle.icon(t, kind === "top" ? 40 : kind === "tile" ? 34 : 22),
        title: circle.title(t),
        meta: circle.host(t.url),
        space: space ? { name: space.name, hue: space.hue } : { name: "Favorites", hue: current.hue },
        hint: "Switch to tab",
        run: () => circle.send({ cmd: "activate", id: t.id }),
      };
    };
    const openUrls = new Set([...tabs.map((t) => t.url), state.nav.url]);
    const closed = state.archive.filter((a) => !openUrls.has(a.url) && (!query || circle.matches(query, a.title, a.url)));
    const closedItem = (a) => ({
      glyph: GLYPHS.closed,
      title: circle.title(a),
      meta: circle.host(a.url),
      hint: "Reopen",
      run: () => circle.send({ cmd: "reopen", url: a.url }),
    });

    if (spotlight && !query) {
      // Just opened: jump to a space or a recent tab.
      if (state.spaces.length > 1) {
        out.push({ group: "Spaces" });
        for (const s of state.spaces) {
          out.push({ kind: "chip", title: s.name, space: { hue: s.hue }, current: s === current, run: () => circle.send({ cmd: "switch_space", id: s.id }) });
        }
      }
      if (tabs.length) out.push({ group: "Recent tabs" }, ...tabs.slice(0, 4).map((t) => tabItem(t, "tile")));
      if (tabs.length > 4) out.push({ group: "Open tabs" }, ...tabs.slice(4, 8).map((t) => tabItem(t)));
      if (closed.length) out.push({ group: "Recently closed" }, ...closed.slice(0, 3).map(closedItem));
      return out;
    }

    const go = query && {
      glyph: looksLikeUrl(query) ? GLYPHS.go : GLYPHS.search,
      title: query,
      meta: looksLikeUrl(query) ? "Open" : "Search the web",
      hint: newTab ? "New tab" : "Open",
      run: () => circle.send({ cmd: "navigate", input: query, new_tab: newTab }),
      runNewTab: () => circle.send({ cmd: "navigate", input: query, new_tab: true }),
    };

    const matches = query ? tabs.filter((t) => circle.matches(query, t.title, t.page_title || "", t.url)) : tabs.slice(0, 6);
    // A tab whose title or site starts with the query is probably where
    // the person wants to go: it becomes the top hit.
    const q = query.toLowerCase();
    // Best first: the site starts with the query, then the title, then any
    // word of the title.
    const strength = (t) => {
      const title = circle.title(t).toLowerCase();
      if (circle.host(t.url).startsWith(q)) return 3;
      if (title.startsWith(q)) return 2;
      return title.split(/[\s\-–—/|:·]+/).some((w) => w.startsWith(q)) ? 1 : 0;
    };
    const best = matches.reduce((a, t) => (strength(t) > (a ? strength(a) : 0) ? t : a), null);
    const top = spotlight && query && !looksLikeUrl(query) ? best : null;
    if (top) out.push({ group: "Top hit" }, tabItem(top, "top"));
    else if (go) out.push({ ...go, kind: "top" });

    const rest = matches.filter((t) => t !== top).slice(0, 6);
    if (rest.length) out.push({ group: "Open tabs" }, ...rest.map((t) => tabItem(t)));

    const commands = query
      ? [
          ...COMMANDS.map(([name, run]) => [typeof name === "function" ? name() : name, run]),
          ...state.spaces.map((s) => [`Go to Space: ${s.name}`, () => circle.send({ cmd: "switch_space", id: s.id })]),
        ].filter(([name]) => circle.matches(query, name))
      : [];
    if (commands.length) out.push({ group: "Commands" });
    for (const [name, run] of commands.slice(0, 5)) out.push({ glyph: GLYPHS.cmd, title: name, hint: "Run", run: () => (close(), run()) });

    const closedCount = mode === "edit" ? 0 : query ? 4 : 5;
    if (closed.length && closedCount) out.push({ group: "Recently closed" }, ...closed.slice(0, closedCount).map(closedItem));

    if (historyHits.query === query && mode !== "edit") {
      // One entry per page, skipping pages already offered above.
      const shown = new Set([...openUrls, ...closed.slice(0, closedCount).map((a) => a.url)]);
      const visited = [];
      for (const v of historyHits.visits) {
        if (shown.has(v.url)) continue;
        shown.add(v.url);
        visited.push(v);
        if (visited.length === 5) break;
      }
      if (visited.length) out.push({ group: "History" });
      for (const v of visited) {
        out.push({
          glyph: GLYPHS.history,
          title: v.title || circle.host(v.url) || v.url,
          meta: [circle.host(v.url), when(v.at)].filter(Boolean).join(" · "),
          hint: newTab ? "New tab" : "Open",
          run: () => circle.send({ cmd: "navigate", input: v.url, new_tab: newTab }),
          runNewTab: () => circle.send({ cmd: "navigate", input: v.url, new_tab: true }),
        });
      }
    }

    // A search or address that isn't the top hit goes last.
    if (go && top) out.push({ group: looksLikeUrl(query) ? "Go to" : "Search the web" }, { ...go, meta: "" });
    return out;
  }

  function when(seconds) {
    const days = Math.floor((Date.now() / 1000 - seconds) / 86400);
    if (days <= 0) return "today";
    if (days === 1) return "yesterday";
    if (days < 7) return `${days} days ago`;
    return new Date(seconds * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }

  // The typed words in bold, wherever they appear.
  function highlight(text) {
    if (!words.length) return circle.esc(text);
    const pattern = new RegExp(words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"), "gi");
    let out = "";
    let last = 0;
    for (const m of text.matchAll(pattern)) {
      out += `${circle.esc(text.slice(last, m.index))}<b>${circle.esc(m[0])}</b>`;
      last = m.index + m[0].length;
    }
    return out + circle.esc(text.slice(last));
  }

  function metaHtml(item) {
    const parts = [];
    if (item.meta) parts.push(`<span class="ellipsis">${circle.esc(item.meta)}</span>`);
    if (item.space) parts.push(`<span class="dot" style="--dot-hue:${item.space.hue}"></span><span>${circle.esc(item.space.name)}</span>`);
    return parts.join("");
  }

  function itemHtml(item, index) {
    const cls = `item ${item.kind || ""} ${index === selected ? "selected" : ""}`;
    const lead = item.icon || `<span class="glyph">${item.glyph || ""}</span>`;
    const enter = `<span class="hint">${circle.esc(item.hint || "")}</span><span class="key enter">↵</span>`;
    switch (item.kind) {
      case "chip":
        return `<div class="${cls}" data-index="${index}"><span class="dot" style="--dot-hue:${item.space.hue}"></span>${circle.esc(item.title)}</div>`;
      case "tile":
        return `<div class="${cls}" data-index="${index}">${lead}<span class="title"><span class="ellipsis">${circle.esc(item.title)}</span><span class="meta">${metaHtml(item)}</span></span></div>`;
      case "top":
        return `<div class="${cls}" data-index="${index}">${lead}<span class="title"><span class="ellipsis">${highlight(item.title)}</span><span class="meta">${metaHtml(item)}</span></span>${enter}</div>`;
      default:
        return `<div class="${cls}" data-index="${index}">${lead}<span class="title ellipsis">${highlight(item.title)}</span><span class="meta">${metaHtml(item)}</span>${enter}</div>`;
    }
  }

  function render() {
    let index = -1;
    let html = "";
    let wrap = null; // the open chips/tiles container
    for (const item of items) {
      const container = item.kind === "chip" ? "chips" : item.kind === "tile" ? "tiles" : null;
      if (wrap && wrap !== container) {
        html += "</div>";
        wrap = null;
      }
      if (item.group) {
        html += `<div class="group">${circle.esc(item.group)}</div>`;
        continue;
      }
      if (container && !wrap) {
        html += `<div class="${container}">`;
        wrap = container;
      }
      index += 1;
      html += itemHtml(item, index);
    }
    if (wrap) html += "</div>";
    list.innerHTML = html;
  }

  function keyHints() {
    const mod = mac() ? "⌘" : "Ctrl+";
    const hints = [["↑↓", "Move"]];
    if (mode === "spotlight" && !input.value.trim()) {
      hints.push(["↵", "Switch"], [mac() ? "⌃1–9" : "Alt+1–9", "Space"]);
    } else {
      hints.push(["↵", "Open"]);
      if (mode === "edit") hints.push([`${mod}↵`, "In new tab"]);
    }
    $("keys").innerHTML = hints.map(([key, label]) => `<span><span class="key">${key}</span>${label}</span>`).join("");
  }

  function refresh() {
    updateCount();
    if (mode === "find") {
      items = [];
      list.innerHTML = "";
    } else {
      words = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
      items = buildItems(input.value);
      const runnable = items.filter((i) => !i.group);
      // Just opened, the first recent tab is selected, not the spaces.
      if (!input.value.trim() && selected === 0) {
        const tile = runnable.findIndex((i) => i.kind === "tile");
        if (tile > 0) selected = tile;
      }
      selected = Math.min(selected, Math.max(runnable.length - 1, 0));
      render();
      keyHints();
    }
    reportHeight();
  }

  // Only the find bar sizes its window to fit; the rest cover the page.
  function reportHeight() {
    if (mode !== "find") return;
    const height = Math.ceil($("panel").getBoundingClientRect().height);
    if (height !== lastHeight) {
      lastHeight = height;
      circle.send({ cmd: "palette_height", height });
    }
  }

  const runnable = () => items.filter((i) => !i.group);

  function runSelected(newTab) {
    const item = runnable()[selected];
    if (!item) return;
    if (newTab && item.runNewTab) item.runNewTab();
    else item.run();
  }

  function moveSelection(delta) {
    const count = runnable().length;
    if (!count) return;
    selected = (selected + delta + count) % count;
    list.querySelectorAll(".item").forEach((el) => el.classList.toggle("selected", Number(el.dataset.index) === selected));
    list.querySelector(".item.selected")?.scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("input", () => {
    selected = 0;
    if (mode === "find") circle.send({ cmd: "find", text: input.value });
    refresh();
    fetchHistory();
  });

  let historyTimer = null;
  function fetchHistory() {
    clearTimeout(historyTimer);
    const query = input.value.trim();
    if (mode === "find" || mode === "edit" || !query) return;
    historyTimer = setTimeout(async () => {
      try {
        const res = await fetch(`circle://ui/api/history?${new URLSearchParams({ q: query, limit: 60 })}`);
        const { visits } = await res.json();
        if (input.value.trim() !== query || !mode) return;
        historyHits = { query, visits };
        refresh();
      } catch {}
    }, 60);
  }

  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (mode === "find") circle.send({ cmd: "find", text: input.value, backward: event.shiftKey });
      else runSelected(event.metaKey || event.ctrlKey);
    } else if (event.key === "ArrowDown" || (event.ctrlKey && event.key === "n")) {
      event.preventDefault();
      moveSelection(1);
    } else if (event.key === "ArrowUp" || (event.ctrlKey && event.key === "p")) {
      event.preventDefault();
      moveSelection(-1);
    }
  });

  list.addEventListener("mousemove", (event) => {
    const el = event.target.closest(".item");
    if (el && Number(el.dataset.index) !== selected) {
      selected = Number(el.dataset.index);
      moveSelection(0);
    }
  });

  list.addEventListener("click", (event) => {
    const el = event.target.closest(".item");
    if (!el) return;
    selected = Number(el.dataset.index);
    runSelected(event.metaKey || event.ctrlKey);
  });

  // Clicking the blurred page closes the command bar.
  $("backdrop").addEventListener("mousedown", close);

  $("prev").onclick = () => circle.send({ cmd: "find", text: input.value, backward: true });
  $("next").onclick = () => circle.send({ cmd: "find", text: input.value });
  $("close").onclick = close;

  // Clicking elsewhere (the sidebar) dismisses it (but not the find bar).
  // Focus moving into the settings frame is not leaving, though.
  window.addEventListener("blur", () => {
    setTimeout(() => {
      if (document.activeElement?.tagName === "IFRAME") return;
      if (mode && mode !== "find" && Date.now() - openedAt > 300) close();
    });
  });

  circle.send({ cmd: "ready", from: "palette" });
})();
