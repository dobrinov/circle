// The sidebar: navigation, favorites, the space's pinned and today tabs,
// spaces, downloads and permission prompts.
(() => {
  const $ = (id) => document.getElementById(id);
  const X_ICON = '<svg viewBox="0 0 12 12"><path d="M3 3l6 6M9 3 3 9"/></svg>';
  const RESET_ICON = '<svg viewBox="0 0 12 12"><path d="M2.5 6a3.5 3.5 0 1 0 1-2.5M2.5 1.5v2h2"/></svg>';
  const LOCK_ICON = '<svg class="lock" viewBox="0 0 12 12"><rect x="2.5" y="5.5" width="7" height="5" rx="1"/><path d="M4 5.5V4a2 2 0 0 1 4 0v1.5"/></svg>';

  let state = null;
  let pending = null;
  let busy = false; // dragging or renaming: defer re-renders
  let lastToast = 0;

  const space = () => state.spaces[state.active_space];
  const spaceTabs = (s) => [...s.folders.flatMap((f) => f.tabs), ...s.pinned, ...s.tabs];
  const allTabs = () => [...state.favorites, ...state.spaces.flatMap(spaceTabs)];
  const CHEVRON = '<svg class="chev" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m3 4.5 3 3 3-3"/></svg>';
  const FOLDER_ICON = '<svg class="folder-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M1.75 4.25a1.5 1.5 0 0 1 1.5-1.5h3l1.5 1.75h5a1.5 1.5 0 0 1 1.5 1.5v5.75a1.5 1.5 0 0 1-1.5 1.5H3.25a1.5 1.5 0 0 1-1.5-1.5z"/></svg>';
  const tabById = (id) => allTabs().find((t) => t.id === id);

  circle.render = (next) => {
    if (busy) {
      pending = next;
      return;
    }
    state = next;
    draw();
  };

  function flushPending() {
    busy = false;
    if (pending) {
      const next = pending;
      pending = null;
      circle.render(next);
    }
  }

  function tabClasses(tab) {
    const classes = [];
    if (tab.id === state.active) classes.push("active");
    if (!tab.live) classes.push("unloaded");
    if (tab.moved) classes.push("moved");
    if (state.split && state.split.includes(tab.id)) classes.push("split");
    return classes.join(" ");
  }

  function folderBlock(folder) {
    return `<div class="folder ${folder.collapsed ? "collapsed" : ""}" data-folder="${folder.id}">
      <div class="tab folder-head" data-folder-head="${folder.id}" title="${circle.esc(folder.name)}">
        ${CHEVRON}${FOLDER_ICON}<span class="title ellipsis">${circle.esc(folder.name)}</span><span class="count">${folder.tabs.length || ""}</span>
      </div>
      <div class="folder-body" data-folder-body="${folder.id}">${folder.tabs.map((t) => tabRow(t, "pinned", folder.id)).join("")}</div>
    </div>`;
  }

  // The format chosen in the recording controls.
  let recordFormat = "video";

  const UNSPLIT_ICON = '<svg viewBox="0 0 12 12" aria-hidden="true"><rect x="1" y="2" width="4" height="8" rx="1"/><rect x="7" y="2" width="4" height="8" rx="1"/></svg>';
  const DEV_BADGE = '<span class="dev" title="Running on this computer">DEV</span>';

  function tabRow(tab, section, folder) {
    const leading = tab.loading && tab.live ? '<span class="spinner"></span>' : circle.icon(tab);
    const unsplit = state.split && state.split.includes(tab.id)
      ? `<button class="x" data-unsplit title="Separate split view">${UNSPLIT_ICON}</button>`
      : "";
    const reset = section === "pinned" ? `<button class="x reset" data-reset="${tab.id}" title="Back to pinned page">${RESET_ICON}</button>` : "";
    const closeTitle = section === "today" ? "Close" : "Unload";
    const tip = tab.renamed ? `${circle.title(tab)}\n${tab.page_title || ""}\n${tab.url}` : `${circle.title(tab)}\n${tab.url}`;
    return `<div class="tab ${tabClasses(tab)}" draggable="true" data-id="${tab.id}" data-section="${section}"${folder ? ` data-in-folder="${folder}"` : ""} title="${circle.esc(tip)}">
      ${leading}<span class="title ellipsis">${circle.esc(circle.shortTitle(tab))}</span>
      ${circle.isLocal(tab.url) ? DEV_BADGE : ""}${unsplit}${reset}<button class="x" data-close="${tab.id}" title="${closeTitle}">${X_ICON}</button>
    </div>`;
  }

  // The same page also runs as the sidebar that peeks out over the page
  // while the real one is collapsed (loaded with ?peek).
  const isPeek = new URLSearchParams(location.search).has("peek");
  if (isPeek) document.body.classList.add("peek");

  circle.animate = (open) => {
    if (!isPeek) document.body.classList.toggle("collapsed", !open);
  };

  function draw() {
    document.body.classList.add(state.platform);
    document.body.style.width = `${state.sidebar_width}px`;
    if (!isPeek) document.body.classList.toggle("collapsed", !state.sidebar_visible);
    document.body.classList.toggle("dimmed", !!state.palette?.backdrop);
    if (isPeek) {
      document.body.classList.toggle("local-frame", circle.isLocal(state.nav.url || ""));
      // The native stripes start at the frame's bottom edge; line up with them.
      document.body.style.setProperty("--stripe-shift", `${innerHeight % 12}px`);
    }
    const current = space();
    circle.setHue(current.hue);

    // Navigation
    $("back").disabled = !state.nav.back;
    $("forward").disabled = !state.nav.forward;
    const url = state.nav.url || "";
    const secure = url.startsWith("https://");
    // Local dev servers show their port, which tells them apart.
    const local = circle.isLocal(url);
    const shown = local ? new URL(url).host : circle.host(url);
    // The site, then the rest of the address, muted.
    let path = "";
    try {
      const parsed = new URL(url);
      if (/^https?:$/.test(parsed.protocol)) path = (parsed.pathname + parsed.search).replace(/\/$/, "");
    } catch {}
    $("url").innerHTML = url
      ? `${secure ? LOCK_ICON : ""}<span class="text ellipsis">${circle.esc(shown)}<span class="path">${circle.esc(path)}</span></span>${local ? DEV_BADGE : ""}`
      : '<span class="text ellipsis" style="color:var(--muted)">Search or enter address</span>';

    drawExtensions();

    // An agent is driving the browser.
    $("agent").innerHTML = state.agent.enabled && state.agent.active
      ? '<span class="pulse"></span><span class="grow"><b>Agent in control</b></span><button data-agent-stop title="Turn off agent control">Stop</button>'
      : "";

    // A recording in progress, with its running time.
    // A bug report being recorded.
    const bugBox = $("bugrec");
    if (!state.bug_report) {
      bugBox.innerHTML = "";
      clearInterval(bugBox.timer);
      bugBox.timer = null;
    } else if (!bugBox.timer) {
      bugBox.innerHTML = `<span class="dot"></span><span class="grow"><b>Bug report</b> <time></time></span><button data-bug-stop title="Stop and write the report">Done</button>`;
      const since = state.bug_report.since;
      const tick = () => {
        const s = Math.max(0, Math.floor((Date.now() - since) / 1000));
        bugBox.querySelector("time").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
      };
      tick();
      bugBox.timer = setInterval(tick, 500);
    }

    const rec = state.recording;
    const recBox = $("recording");
    const prompt = !rec && state.record_prompt;
    recBox.classList.toggle("prompt", !!prompt);
    if (prompt) {
      // The controls: nothing records until Start is pressed.
      clearInterval(recBox.timer);
      recBox.timer = null;
      if (recBox.dataset.shown !== "prompt") {
        recordFormat = prompt.format;
        recBox.dataset.shown = "prompt";
      }
      recBox.innerHTML = `<div class="rec-head"><span class="grow">Record this tab</span>
          <button class="rec-cancel" data-record-cancel title="Cancel" aria-label="Cancel">${X_ICON}</button></div>
        ${prompt.tab ? `<div class="rec-tab ellipsis">${circle.esc(prompt.tab)}</div>` : ""}
        <div class="seg rec-format" role="radiogroup" aria-label="Format">
          <button role="radio" data-record-format="video" class="${recordFormat === "video" ? "on" : ""}" aria-checked="${recordFormat === "video"}">Video</button>
          <button role="radio" data-record-format="gif" class="${recordFormat === "gif" ? "on" : ""}" aria-checked="${recordFormat === "gif"}">GIF</button>
        </div>
        <div class="rec-marks">
          <button data-record-mark="clicks" aria-pressed="${!!prompt.clicks}" title="Show the pointer and clicks in the recording">Clicks</button>
          <button data-record-mark="keys" aria-pressed="${!!prompt.keys}" title="Show the keys you press in the recording">Keys</button>
        </div>
        <button class="rec-start" data-record-start><span class="dot"></span>Start recording</button>`;
    } else if (!rec || rec.saving) {
      recBox.dataset.shown = "";
      clearInterval(recBox.timer);
      recBox.timer = null;
      recBox.innerHTML = rec
        ? `<span class="spinner"></span><span class="grow">Saving ${rec.format === "gif" ? "GIF" : "video"}…</span>`
        : "";
    } else if (!recBox.timer) {
      recBox.dataset.shown = "";
      const label = rec.format === "gif" ? "Recording GIF" : "Recording";
      recBox.innerHTML = `<span class="dot"></span><span class="grow"><b>${label}</b> <time></time></span><button data-record-stop title="Stop and save to Downloads">Stop</button>`;
      const tick = () => {
        const s = Math.max(0, Math.floor((Date.now() - rec.since) / 1000));
        recBox.querySelector("time").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
      };
      tick();
      recBox.timer = setInterval(tick, 500);
    }

    // Permission prompts
    $("prompts").innerHTML = state.prompts
      .map(
        (p) => `<div class="card"><b>${circle.esc(circle.host(p.origin))}</b> wants to use ${circle.esc(p.kinds.join(", "))}.
          <div class="row"><button data-deny="${p.id}">Block</button><button class="primary" data-allow="${p.id}">Allow</button></div></div>`,
      )
      .join("");

    // Favorites
    const favs = $("favorites");
    favs.classList.toggle("empty", state.favorites.length === 0);
    circle.html(favs, state.favorites.length
      ? state.favorites
          .map(
            (t) => `<div class="fav ${tabClasses(t)}" draggable="true" data-id="${t.id}" data-section="favorites" title="${circle.esc(circle.title(t))}">${circle.icon(t, 20)}</div>`,
          )
          .join("")
      : "");

    // Space
    document.body.classList.toggle("private", !!current.private);
    $("space-title").innerHTML = current.private
      ? `<span class="private-emoji" aria-hidden="true">🙈</span><span class="grow">Fap mode</span>
         <button class="end-private" data-end-private title="Close every tab here and forget them (⇧⌘.)">End</button>`
      : `${current.icon ? `<span>${circle.esc(current.icon)}</span>` : '<span class="space-mark" aria-hidden="true"></span>'}
      <span class="grow ellipsis" id="space-name" title="Double-click to rename">${circle.esc(current.name)}</span>
      <button class="muted-btn" data-space-menu="${current.id}">•••</button>`;
    $("private-note").hidden = !current.private;

    circle.html(
      $("pinned"),
      current.folders.map(folderBlock).join("") + current.pinned.map((t) => tabRow(t, "pinned")).join(""),
    );
    const today = current.tabs.map((t) => tabRow(t, "today")).join("");
    circle.html($("today"), today);
    // Labels only where there is something to label.
    $("pinned-label").hidden = !current.pinned.length && !current.folders.length;
    $("today-label").hidden = !current.tabs.length;
    $("new-tab-key").textContent = state.platform === "macos" ? "⌘T" : "Ctrl+T";

    // A download in progress, in the tray.
    const active = (state.downloads || []).find((d) => !d.complete && !d.failed);
    $("dlprog").innerHTML = active
      ? `<svg class="state" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2.5v7m0 0L5 6.5m3 3 3-3"/><path d="M2.5 10v1.5a2 2 0 0 0 2 2h7a2 2 0 0 0 2-2V10"/></svg>
         <span class="grow"><span style="display:flex;gap:6px"><b class="ellipsis">${circle.esc(active.name)}</b><span class="meta">${Math.max(active.percent, 0)}%</span></span><div class="bar"><i style="width:${Math.max(active.percent, 2)}%"></i></div></span>`
      : "";
    $("library-btn").classList.toggle("progress", !!active);
    $("library-btn").style.setProperty("--p", active ? Math.max(active.percent, 3) : 0);
    $("new-tab").classList.toggle("active", false);

    renderLibrary();

    // A new version of Circle.
    const update = state.update?.status;
    const card = (line, action = "") =>
      `<span class="mark" aria-hidden="true"></span><span class="grow"><b>Circle ${circle.esc(update.version)}</b><span class="line">${line}</span></span>${action}`;
    $("update").innerHTML =
      update?.status === "ready"
        ? card("Restart to finish updating", `<button data-update-restart title="Restart Circle to finish updating">Restart</button>`)
        : update?.status === "downloading"
          ? card(`Downloading… ${update.percent}%<span class="bar"><i style="width:${update.percent}%"></i></span>`)
          : update?.status === "available" && update.installable
            ? card("A new version is out", `<button data-update-install>Update</button>`)
            : update?.status === "available"
              ? card("Download it to update", `<button data-update-page="${circle.esc(update.url)}">Download</button>`)
              : "";

    // Spaces
    $("spaces").innerHTML = state.spaces
      .map(
        (s, i) => `<button class="space-dot ${i === state.active_space ? "active" : ""} ${s.private ? "private-dot" : ""}" style="--dot: hsl(${s.hue} 55% 58%)" data-space="${s.id}" title="${s.private ? "Fap mode" : `${circle.esc(s.name)} (⌃${i + 1})`}">${
          s.private ? '<span class="private-emoji" aria-hidden="true">🙈</span>' : s.icon ? circle.esc(s.icon) : "<i></i>"
        }</button>`,
      )
      .join("");

    // Toast
    if (state.toast && state.toast.nonce !== lastToast) {
      lastToast = state.toast.nonce;
      const toast = $("toast");
      toast.textContent = state.toast.text;
      toast.classList.add("show");
      clearTimeout(toast.timer);
      toast.timer = setTimeout(() => toast.classList.remove("show"), 1600);
    }
  }

  // ----- clicks ------------------------------------------------------------

  // While dimmed behind the command bar, a click only closes it.
  document.addEventListener(
    "mousedown",
    (event) => {
      if (!document.body.classList.contains("dimmed")) return;
      event.preventDefault();
      event.stopPropagation();
      circle.send({ cmd: "close_palette" });
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      if (document.body.classList.contains("dimmed")) event.stopPropagation();
    },
    true,
  );

  document.addEventListener("click", (event) => {
    if (event.target.closest("[data-agent-stop]")) return circle.action("toggle_agent_access");
    if (event.target.closest("[data-end-private]")) return circle.action("end_private");
    if (event.target.closest("[data-clear-today]")) return circle.action("clear_today");
    if (event.target.closest("#dlprog")) return $("library-btn").click();
    if (event.target.closest("[data-update-restart]")) return circle.action("restart_to_update");
    if (event.target.closest("[data-update-install]")) return circle.action("check_for_updates");
    const updatePage = event.target.closest("[data-update-page]");
    if (updatePage) return circle.send({ cmd: "navigate", input: updatePage.dataset.updatePage, new_tab: true });
    if (event.target.closest("[data-record-stop]")) return circle.action("stop_recording");
    if (event.target.closest("[data-record-cancel]")) return circle.action("cancel_recording");
    if (event.target.closest("[data-record-start]")) return circle.action({ start_recording: recordFormat });
    const mark = event.target.closest("[data-record-mark]");
    if (mark) {
      return circle.send({ cmd: "settings", [`recording_${mark.dataset.recordMark}`]: mark.getAttribute("aria-pressed") !== "true" });
    }
    const format = event.target.closest("[data-record-format]");
    if (format) {
      recordFormat = format.dataset.recordFormat;
      for (const b of document.querySelectorAll("[data-record-format]")) {
        b.classList.toggle("on", b === format);
        b.setAttribute("aria-checked", String(b === format));
      }
      return;
    }
    if (event.target.closest("[data-bug-stop]")) return circle.send({ cmd: "bug_report_stop" });
    if (event.target.closest("[data-unsplit]")) return circle.action("unsplit");
    const target = event.target.closest("[data-close],[data-reset],[data-allow],[data-deny],[data-space],[data-space-menu],[data-download],.tab,.fav");
    hideMenu();
    if (!target) return;
    const d = target.dataset;
    if (d.close) return circle.send({ cmd: "close", id: Number(d.close) });
    if (d.reset) return circle.send({ cmd: "reset", id: Number(d.reset) });
    if (d.allow) return circle.send({ cmd: "permission", id: Number(d.allow), allow: true });
    if (d.deny) return circle.send({ cmd: "permission", id: Number(d.deny), allow: false });
    if (d.space) return circle.send({ cmd: "switch_space", id: Number(d.space) });
    if (d.spaceMenu) return showSpaceMenu(event);
    if (d.download) return circle.send({ cmd: "reveal_download", id: Number(d.download) });
    if (d.folderHead) return circle.send({ cmd: "toggle_folder", id: Number(d.folderHead) });
    if (target.id === "new-tab") return circle.action({ open_palette: "new_tab" });
    if (d.id) return circle.send({ cmd: "activate", id: Number(d.id) });
  });

  // Middle-click closes, like tab strips everywhere.
  document.addEventListener("auxclick", (event) => {
    const tab = event.target.closest(".tab[data-id]");
    if (event.button === 1 && tab) circle.send({ cmd: "close", id: Number(tab.dataset.id) });
  });

  $("toggle").onclick = () => circle.action("toggle_sidebar");
  $("back").onclick = () => circle.action("back");
  $("forward").onclick = () => circle.action("forward");
  $("reload").onclick = () => circle.action(state?.nav.loading ? "stop" : "reload");
  $("url").onclick = () => circle.action({ open_palette: "edit" });

  // ----- extension buttons ---------------------------------------------------

  const PUZZLE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6.5 2.5h3v2a1.25 1.25 0 0 0 2.5 0h1.5v3.5h-1.5a1.25 1.25 0 0 0 0 2.5h1.5v3h-3.5v-1.5a1.25 1.25 0 0 0-2.5 0v1.5H2.5v-3.5h1.5a1.25 1.25 0 0 0 0-2.5H2.5V4.5h4z"/></svg>';
  const MAX_PINNED = 3;
  const extIcon = (e) => `<img src="circle://ui/ext-icon/${circle.esc(e.id)}" alt="">`;
  function drawExtensions() {
    const list = state.extensions || [];
    const pinned = list.filter((e) => e.pinned).slice(0, MAX_PINNED);
    circle.html(
      $("exts"),
      list.length
        ? pinned
            .map((e) => `<button data-ext="${circle.esc(e.id)}" class="${e.open ? "open" : ""}" title="${circle.esc(e.title)}" aria-label="${circle.esc(e.title)}">${extIcon(e)}</button>`)
            .join("") + `<button data-ext-menu title="Extensions" aria-label="Extensions">${PUZZLE}</button>`
        : "",
    );
  }
  const openExtension = (id, anchor) => {
    const r = anchor.getBoundingClientRect();
    circle.send({ cmd: "ext_open", id, x: Math.round(r.left), y: Math.round(r.top), width: Math.round(r.width), height: Math.round(r.height), peek: isPeek });
  };
  function showExtensionMenu(button) {
    const list = state.extensions || [];
    menu.className = "exts";
    menu.innerHTML =
      '<div class="label">Extensions</div>' +
      list
        .map((e) => `<button class="ext" data-ext-row="${circle.esc(e.id)}">${extIcon(e)}<span class="name">${circle.esc(e.name)}</span>
          <span class="pin" data-ext-pin="${circle.esc(e.id)}" data-pinned="${e.pinned}" title="${e.pinned ? "Unpin" : "Pin"}">${e.pinned ? "Unpin" : "Pin"}</span></button>`)
        .join("") +
      '<hr><button data-ext-manage>Manage extensions</button>';
    menu.classList.add("open");
    const r = button.getBoundingClientRect();
    menu.style.left = `${Math.max(6, Math.min(r.right - menu.offsetWidth, innerWidth - menu.offsetWidth - 6))}px`;
    menu.style.top = `${r.bottom + 4}px`;
    menu.onclick = (e) => {
      e.stopPropagation();
      const pin = e.target.closest("[data-ext-pin]");
      const row = e.target.closest("[data-ext-row]");
      hideMenu();
      if (pin) return circle.send({ cmd: "ext_pin", id: pin.dataset.extPin, pinned: pin.dataset.pinned !== "true" });
      if (row) return openExtension(row.dataset.extRow, button);
      if (e.target.closest("[data-ext-manage]")) circle.send({ cmd: "ext_manage" });
    };
  }
  $("exts").addEventListener("click", (event) => {
    event.stopPropagation();
    const button = event.target.closest("button");
    if (!button) return;
    if (button.hasAttribute("data-ext-menu")) {
      if (menu.classList.contains("open") && menu.classList.contains("exts")) return hideMenu();
      return showExtensionMenu(button);
    }
    hideMenu();
    openExtension(button.dataset.ext, button);
  });
  // ----- the library: downloads and recently closed ---------------------------

  let libraryView = "downloads";
  // Finished downloads already seen in the open panel; newer ones get a dot.
  const seenDownloads = new Set();
  let libraryPrimed = false;

  const FOLDER_ICON_SM = '<svg viewBox="0 0 16 16"><path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h3l1.5 1.5h4.5A1.5 1.5 0 0 1 14 6.5v5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z"/></svg>';

  function ago(seconds) {
    const s = Math.max(0, Date.now() / 1000 - seconds);
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return new Date(seconds * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }

  function openLibrary(open) {
    document.body.classList.toggle("library", open);
    $("library").setAttribute("aria-hidden", String(!open));
    $("library-btn").setAttribute("aria-expanded", String(open));
    renderLibrary();
  }

  function renderLibrary() {
    if (!state) return;
    const downloads = state.downloads || [];
    // The first render only remembers what's there: nothing is "new" yet.
    if (!libraryPrimed) {
      for (const d of downloads) seenDownloads.add(d.id);
      libraryPrimed = true;
    }
    const open = document.body.classList.contains("library");
    if (open && libraryView === "downloads") for (const d of downloads) if (d.complete || d.failed) seenDownloads.add(d.id);
    const active = downloads.some((d) => !d.complete && !d.failed);
    const unseen = downloads.some((d) => (d.complete || d.failed) && !seenDownloads.has(d.id));
    // In progress shows as a ring around the button; the dot is for new files.
    $("library-badge").classList.toggle("on", unseen);

    for (const b of document.querySelectorAll("[data-lib-view]")) {
      const on = b.dataset.libView === libraryView;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", String(on));
    }
    if (!open) return;

    let body = "";
    let foot = "";
    if (libraryView === "downloads") {
      body = downloads
        .map((d) => {
          const ext = (d.name.match(/\.([a-z0-9]{1,5})$/i) || [])[1] || "file";
          const status = d.failed ? "Failed" : d.complete ? ago(d.at) : `${Math.max(d.percent, 0)}% · downloading`;
          const bar = d.complete || d.failed ? "" : `<div class="bar"><i style="width:${Math.max(d.percent, 2)}%"></i></div>`;
          return `<div class="lib-item ${d.failed ? "failed" : ""}" data-download="${d.id}" title="${circle.esc(d.path)}">
            <span class="ext">${circle.esc(ext)}</span>
            <div class="info"><div class="name ellipsis">${circle.esc(d.name)}</div><small>${status}</small>${bar}</div>
            <button class="reveal" data-download="${d.id}" title="Show in folder" aria-label="Show in folder">${FOLDER_ICON_SM}</button>
          </div>`;
        })
        .join("");
      if (!body) body = '<div class="lib-empty">Downloads and recordings show up here.</div>';
      if (downloads.some((d) => d.complete || d.failed)) foot = '<button id="lib-clear">Clear list</button>';
    } else {
      body = (state.archive || [])
        .slice(0, 50)
        .map(
          (a) => `<div class="lib-item" data-reopen="${circle.esc(a.url)}" title="${circle.esc(a.url)}">
            ${circle.icon(a)}
            <div class="info"><div class="name ellipsis">${circle.esc(circle.title(a))}</div><small>${circle.esc(circle.host(a.url))} · ${ago(a.closed_at)}</small></div>
          </div>`,
        )
        .join("");
      if (!body) body = '<div class="lib-empty">Tabs you close show up here.</div>';
    }
    $("lib-body").innerHTML = body;
    $("lib-foot").innerHTML = foot;
  }

  $("library-btn").onclick = () => openLibrary(!document.body.classList.contains("library"));
  $("lib-close").onclick = () => openLibrary(false);
  $("library").addEventListener("click", (event) => {
    const view = event.target.closest("[data-lib-view]");
    if (view) {
      libraryView = view.dataset.libView;
      return renderLibrary();
    }
    if (event.target.closest("#lib-clear")) return circle.send({ cmd: "clear_downloads" });
    const reopen = event.target.closest("[data-reopen]");
    if (reopen) {
      circle.send({ cmd: "reopen", url: reopen.dataset.reopen });
      return openLibrary(false);
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && document.body.classList.contains("library")) openLibrary(false);
  });
  // Times like "3 min ago" stay current while the panel is open.
  setInterval(() => document.body.classList.contains("library") && renderLibrary(), 30000);
  $("add-space").onclick = () => circle.send({ cmd: "new_space" });

  // ----- renaming tabs and folders -------------------------------------------

  // Replaces a row's title with a text field; Enter saves, Escape cancels.
  function editTitle(row, initial, save) {
    const label = row.querySelector(".title");
    if (!label) return;
    busy = true;
    row.draggable = false;
    const input = document.createElement("input");
    input.className = "rename";
    input.spellcheck = false;
    input.value = initial;
    label.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (commit) => {
      if (done) return;
      done = true;
      if (commit && input.value.trim() !== initial.trim()) save(input.value.trim());
      // The row was edited in place: redraw it even if nothing changed.
      const list = row.closest("#pinned, #today, #favorites");
      if (list) list.circleHtml = null;
      flushPending();
      if (!pending) draw();
    };
    input.onkeydown = (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    };
    input.onclick = (e) => e.stopPropagation();
    input.onblur = () => finish(true);
  }

  function renameTab(id) {
    const row = document.querySelector(`.tab[data-id="${id}"]`);
    const tab = tabById(id);
    if (row && tab) editTitle(row, circle.title(tab), (name) => circle.send({ cmd: "rename_tab", id, name }));
  }

  function renameFolder(id) {
    const row = document.querySelector(`[data-folder-head="${id}"]`);
    const folder = space().folders.find((f) => f.id === id);
    if (row && folder) editTitle(row, folder.name, (name) => name && circle.send({ cmd: "rename_folder", id, name }));
  }

  document.addEventListener("dblclick", (event) => {
    if (!event.target.closest(".title")) return;
    const head = event.target.closest("[data-folder-head]");
    if (head) return renameFolder(Number(head.dataset.folderHead));
    const row = event.target.closest(".tab[data-id]");
    if (row) renameTab(Number(row.dataset.id));
  });

  // ----- renaming spaces ---------------------------------------------------

  $("space-title").addEventListener("dblclick", (event) => {
    if (!event.target.closest("#space-name")) return;
    renameSpace(space());
  });

  function renameSpace(target) {
    busy = true;
    const title = $("space-title");
    title.innerHTML = '<input id="space-input" spellcheck="false">';
    const input = $("space-input");
    input.value = target.icon ? `${target.icon} ${target.name}` : target.name;
    input.focus();
    input.select();
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      if (save) {
        // A leading emoji becomes the space icon.
        const match = input.value.trim().match(/^(\p{Extended_Pictographic}️?)\s*(.*)$/u);
        const icon = match ? match[1] : "";
        const name = match ? match[2] || target.name : input.value;
        circle.send({ cmd: "rename_space", id: target.id, name, icon });
      }
      flushPending();
      if (!pending) draw();
    };
    input.onkeydown = (e) => {
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    };
    input.onblur = () => finish(true);
  }

  // ----- context menus -----------------------------------------------------

  const menu = $("menu");

  // ----- space color --------------------------------------------------------

  const picker = $("picker");
  const HUES = [0, 18, 32, 48, 95, 130, 160, 184, 205, 229, 262, 290];

  function showColorPicker(target) {
    picker.innerHTML = `<div class="swatches">${HUES.map(
      (h) => `<button class="swatch" data-hue="${h}" style="--h: ${h}" aria-label="Hue ${h}"></button>`,
    ).join("")}</div>
      <input class="hue" type="range" min="0" max="359" value="${target.hue}" aria-label="Hue">
      <button class="random">Random</button>`;
    const slider = picker.querySelector(".hue");
    let frame = null;
    const setHue = (hue) => {
      slider.value = hue;
      picker.querySelectorAll(".swatch").forEach((s) => s.classList.toggle("selected", Number(s.dataset.hue) === hue));
      // Apply at most once per frame while dragging; the whole window recolors live.
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => circle.send({ cmd: "set_space_hue", id: target.id, hue }));
    };
    picker.querySelectorAll(".swatch").forEach((s) => s.classList.toggle("selected", Number(s.dataset.hue) === target.hue));
    picker.onclick = (e) => {
      e.stopPropagation();
      const swatch = e.target.closest("[data-hue]");
      if (swatch) setHue(Number(swatch.dataset.hue));
      if (e.target.closest(".random")) setHue(Math.floor(Math.random() * 360));
    };
    slider.oninput = () => setHue(Number(slider.value));
    const anchor = $("space-title").getBoundingClientRect();
    picker.style.left = `${Math.max(8, Math.min(anchor.left, innerWidth - 228))}px`;
    picker.style.top = `${anchor.bottom + 6}px`;
    picker.classList.add("open");
  }

  const hidePicker = () => picker.classList.remove("open");
  document.addEventListener("click", (e) => {
    if (!e.target.closest("#picker")) hidePicker();
  });
  document.addEventListener("keydown", (e) => e.key === "Escape" && hidePicker());

  function hideMenu() {
    menu.classList.remove("open");
  }

  function showMenu(event, items) {
    event.preventDefault();
    menu.className = "";
    menu.innerHTML = items
      .map((item) =>
        item === "-"
          ? "<hr>"
          : item.label
            ? `<div class="label">${circle.esc(item.label)}</div>`
            : `<button data-index="${items.indexOf(item)}">${circle.esc(item.text)}</button>`,
      )
      .join("");
    menu.classList.add("open");
    const x = Math.min(event.clientX, innerWidth - menu.offsetWidth - 6);
    const y = Math.min(event.clientY, innerHeight - menu.offsetHeight - 6);
    menu.style.left = `${Math.max(6, x)}px`;
    menu.style.top = `${Math.max(6, y)}px`;
    menu.onclick = (e) => {
      const button = e.target.closest("button[data-index]");
      if (!button) return;
      e.stopPropagation();
      hideMenu();
      items[Number(button.dataset.index)].run();
    };
  }

  document.addEventListener("keydown", (e) => e.key === "Escape" && hideMenu());
  window.addEventListener("blur", hideMenu);

  document.addEventListener("contextmenu", (event) => {
    const head = event.target.closest("[data-folder-head]");
    if (head) {
      const folder = Number(head.dataset.folderHead);
      return showMenu(event, [
        { text: "Rename…", run: () => renameFolder(folder) },
        { text: "Delete Folder", run: () => circle.send({ cmd: "delete_folder", id: folder }) },
      ]);
    }
    const el = event.target.closest("[data-id]");
    if (!el) {
      event.preventDefault();
      return;
    }
    const id = Number(el.dataset.id);
    const section = el.dataset.section;
    const tab = tabById(id);
    const move = (to, index = 0) => () => circle.send({ cmd: "move", id, section: to, index });
    const inFolder = el.dataset.inFolder ? Number(el.dataset.inFolder) : null;
    const items = [{ text: "Rename…", run: () => renameTab(id) }];
    if (tab?.renamed) items.push({ text: "Reset Name", run: () => circle.send({ cmd: "rename_tab", id, name: "" }) });
    items.push("-");
    if (section !== "favorites") items.push({ text: "Add to Favorites", run: move("favorites", state.favorites.length) });
    if (section !== "pinned") items.push({ text: "Pin Tab", run: move("pinned", space().pinned.length) });
    if (section !== "today") items.push({ text: section === "favorites" ? "Remove from Favorites" : "Unpin Tab", run: move("today") });
    if (section !== "favorites") {
      const folders = space().folders.filter((f) => f.id !== inFolder);
      items.push("-", { label: "Move to Folder" });
      for (const f of folders) items.push({ text: f.name, run: () => circle.send({ cmd: "move_to_folder", id, folder: f.id }) });
      items.push({ text: "New Folder with Tab", run: () => circle.send({ cmd: "new_folder", tab: id }) });
      if (inFolder) items.push({ text: "Remove from Folder", run: () => circle.send({ cmd: "move_to_folder", id, folder: null }) });
    }
    items.push("-");
    items.push({ text: "Copy Link", run: () => circle.send({ cmd: "copy_url", id }) });
    items.push({ text: "Duplicate", run: () => circle.send({ cmd: "duplicate", id }) });
    if (state.split && state.split.includes(id)) {
      items.push({ text: "Separate Split View", run: () => circle.action("unsplit") });
    } else if (state.active && state.active !== id) {
      items.push({ text: "Open in Split View", run: () => circle.send({ cmd: "split", id }) });
      items.push({ text: "Open in Split View Below", run: () => circle.send({ cmd: "split", id, stacked: true }) });
    }
    if (tab && tab.live && tab.id !== state.active) items.push({ text: "Unload Tab", run: () => circle.send({ cmd: "unload", id }) });
    const others = state.spaces.filter((s, i) => i !== state.active_space);
    if (others.length && section !== "favorites") {
      items.push("-", { label: "Move to Space" });
      for (const s of others) items.push({ text: `${s.icon ? s.icon + " " : ""}${s.name}`, run: () => circle.send({ cmd: "move_to_space", id, space: s.id }) });
    }
    items.push("-");
    items.push({ text: section === "today" ? "Close Tab" : "Unload & Reset", run: () => circle.send({ cmd: "close", id }) });
    showMenu(event, items);
  });

  function showSpaceMenu(event) {
    const current = space();
    const items = [
      { text: "Rename…", run: () => renameSpace(current) },
      { text: "Change Color…", run: () => showColorPicker(current) },
      { text: "New Folder", run: () => circle.send({ cmd: "new_folder" }) },
      { text: "New Space", run: () => circle.send({ cmd: "new_space" }) },
      "-",
      { text: "Clear Today's Tabs", run: () => circle.action("clear_today") },
    ];
    if (state.spaces.length > 1) items.push({ text: "Delete Space", run: () => circle.send({ cmd: "delete_space", id: current.id }) });
    event.stopPropagation();
    showMenu(event, items);
  }

  // ----- drag and drop -----------------------------------------------------

  let dragId = null;
  let dropLine = null;

  const clearDrop = () => {
    dropLine?.remove();
    dropLine = null;
    document.querySelectorAll(".drop-target").forEach((el) => el.classList.remove("drop-target"));
  };

  document.addEventListener("dragstart", (event) => {
    const el = event.target.closest("[data-id]");
    if (!el) return;
    busy = true;
    dragId = Number(el.dataset.id);
    el.classList.add("dragging");
    event.dataTransfer.effectAllowed = "move";
    const tab = tabById(dragId);
    if (tab) event.dataTransfer.setData("text/uri-list", tab.url);
    // Circle offers a drop card over the page for split view.
    circle.send({ cmd: "tab_drag", id: dragId });
  });

  document.addEventListener("dragend", () => {
    if (dragId !== null) circle.send({ cmd: "tab_drag", id: null });
    dragId = null;
    clearDrop();
    document.querySelectorAll(".dragging").forEach((el) => el.classList.remove("dragging"));
    flushPending();
    if (!pending) draw();
  });

  // Where in a list of rows the pointer is: the index to insert at.
  function insertionIndex(children, event, vertical = true) {
    for (let i = 0; i < children.length; i++) {
      const r = children[i].getBoundingClientRect();
      const before = vertical ? event.clientY < r.top + r.height / 2 : event.clientX < r.left + r.width / 2 && event.clientY < r.bottom;
      if (before) return i;
    }
    return children.length;
  }

  function dropPoint(event) {
    const spaceDot = event.target.closest("[data-space]");
    if (spaceDot) return { space: Number(spaceDot.dataset.space), el: spaceDot };
    const folderEl = event.target.closest("[data-folder]");
    if (folderEl) {
      const folder = Number(folderEl.dataset.folder);
      const body = folderEl.querySelector("[data-folder-body]");
      const onHead = event.target.closest("[data-folder-head]");
      const children = [...body.querySelectorAll(":scope > [data-id]")].filter((c) => Number(c.dataset.id) !== dragId);
      const index = onHead ? children.length : insertionIndex(children, event);
      return { folder, onHead, head: folderEl.querySelector("[data-folder-head]"), container: body, children, index };
    }
    const container = event.target.closest("[data-section]:not(.tab):not(.fav)") ||
      event.target.closest(".tab,.fav")?.parentElement;
    if (!container || !container.dataset.section) return null;
    const children = [...container.querySelectorAll(":scope > [data-id]")].filter((c) => Number(c.dataset.id) !== dragId);
    const index = insertionIndex(children, event, container.dataset.section !== "favorites");
    return { section: container.dataset.section, index, container, children };
  }

  document.addEventListener("dragover", (event) => {
    if (dragId === null) return;
    const point = dropPoint(event);
    clearDrop();
    if (!point) return;
    event.preventDefault();
    if (point.space !== undefined) {
      point.el.classList.add("drop-target");
      return;
    }
    if (point.folder !== undefined && point.onHead) {
      point.head.classList.add("drop-target");
      return;
    }
    if (point.section === "favorites") {
      point.container.classList.add("drop-target");
      return;
    }
    dropLine = document.createElement("div");
    dropLine.className = "drop-line";
    const before = point.children[point.index];
    point.container.insertBefore(dropLine, before || null);
  });

  document.addEventListener("drop", (event) => {
    if (dragId === null) return;
    event.preventDefault();
    const point = dropPoint(event);
    const id = dragId;
    clearDrop();
    if (!point) return;
    if (point.space !== undefined) {
      if (point.space !== space().id) circle.send({ cmd: "move_to_space", id, space: point.space });
      return;
    }
    const before = point.children[point.index] ? Number(point.children[point.index].dataset.id) : null;
    if (point.folder !== undefined) {
      circle.send({ cmd: "move_to_folder", id, folder: point.folder, before });
      return;
    }
    // Tabs are placed relative to their neighbours: the lists the sidebar
    // shows can differ from the stored order (folders, other spaces).
    const index = before === null ? 1e6 : point.index;
    circle.send({ cmd: "move", id, section: point.section, space: space().id, index, before });
  });

  // ----- swipe between spaces ----------------------------------------------

  let swipe = 0;
  let swipeLock = false;
  document.addEventListener(
    "wheel",
    (event) => {
      if (Math.abs(event.deltaX) <= Math.abs(event.deltaY) || swipeLock) return;
      swipe += event.deltaX;
      if (Math.abs(swipe) > 140) {
        circle.action(swipe > 0 ? "next_space" : "prev_space");
        swipe = 0;
        swipeLock = true;
        setTimeout(() => (swipeLock = false), 600);
      }
    },
    { passive: true },
  );

  // ----- theme -------------------------------------------------------------

  const dark = matchMedia("(prefers-color-scheme: dark)");
  const sendTheme = () => circle.send({ cmd: "theme", dark: dark.matches });
  if (!isPeek) {
    dark.addEventListener("change", sendTheme);
    sendTheme();
  }

  // ----- peeking ---------------------------------------------------------------

  if (isPeek) {
    for (const button of document.querySelectorAll("#lights button")) {
      button.addEventListener("click", () => circle.send({ cmd: "window_control", op: button.dataset.op }));
    }
    let closeTimer = null;
    const busyUi = () =>
      menu.classList.contains("open") || picker.classList.contains("open") || document.querySelector("input.rename");
    document.documentElement.addEventListener("mouseenter", () => {
      clearTimeout(closeTimer);
      circle.send({ cmd: "peek", open: true });
    });
    document.documentElement.addEventListener("mouseleave", () => {
      clearTimeout(closeTimer);
      closeTimer = setTimeout(() => {
        if (!busyUi()) circle.send({ cmd: "peek", open: false });
      }, 350);
    });
  }

  circle.send({ cmd: "ready", from: isPeek ? "peek" : "sidebar" });
})();
