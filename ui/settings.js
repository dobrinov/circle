// The settings page. Circle pushes the current values with
// `circle.render(settings)`; changes go back as `{cmd: "settings", ...}`.
// Loaded with ?panel it is the panel over the browser (inside the command
// bar's overlay), which Esc closes.
(() => {
  const $ = (id) => document.getElementById(id);
  const mac = navigator.platform.startsWith("Mac");
  const mod = mac ? "⌘" : "Ctrl+";
  const update = (fields) => circle.send({ cmd: "settings", ...fields });
  const panel = new URLSearchParams(location.search).has("panel");
  document.body.classList.toggle("panel", panel);

  // ----- sections and search ---------------------------------------------------

  const sections = [...document.querySelectorAll("section")];
  const links = [...document.querySelectorAll("nav a")];
  const findInput = $("find-input");

  function showSection(id) {
    for (const section of sections) section.classList.toggle("on", section.id === id);
    for (const link of links) link.classList.toggle("on", link.hash === `#${id}`);
    $("title").textContent = $(id).dataset.title;
    $("content").scrollTop = 0;
  }
  for (const link of links) {
    link.addEventListener("click", (event) => {
      event.preventDefault();
      findInput.value = "";
      filter();
      showSection(link.hash.slice(1));
    });
  }

  // Searching shows every matching row, grouped by section.
  function filter() {
    const query = findInput.value.trim().toLowerCase();
    document.body.classList.toggle("searching", !!query);
    let any = false;
    for (const section of sections) {
      let hits = 0;
      const sectionHit = section.dataset.title.toLowerCase().includes(query);
      for (const row of section.querySelectorAll(".row")) {
        if (row.id === "custom-search-row") continue;
        const hit = !query || sectionHit || row.textContent.toLowerCase().includes(query);
        row.style.display = hit ? "" : "none";
        if (hit) hits++;
      }
      const match = !!query && (hits > 0 || sectionHit);
      section.classList.toggle("match", match);
      any ||= match;
      const link = links.find((l) => l.hash === `#${section.id}`);
      link?.classList.toggle("dim", !!query && !match);
    }
    document.body.classList.toggle("nothing", !!query && !any);
    $("title").textContent = query ? "Search results" : (sections.find((s) => s.classList.contains("on"))?.dataset.title ?? "Settings");
  }
  findInput.addEventListener("input", filter);
  showSection(sections[0].id);

  const close = () => circle.send({ cmd: "close_palette" });
  document.addEventListener("keydown", (event) => {
    if (!panel) return;
    if (event.key === "Escape") {
      event.preventDefault();
      if (findInput.value) {
        findInput.value = "";
        filter();
      } else close();
    } else if ((event.metaKey || event.ctrlKey) && event.key === "f") {
      event.preventDefault();
      findInput.focus();
    }
  });
  document.querySelector(".esc").addEventListener("click", close);
  // Clicking outside Circle's overlay (the sidebar, a page) dismisses the panel.
  if (panel) {
    window.addEventListener("blur", () => {
      setTimeout(() => {
        try {
          // Focus went somewhere other than the overlay around the panel.
          if (parent.document.activeElement?.tagName !== "IFRAME" && !parent.document.hasFocus()) close();
        } catch {}
      });
    });
  }
  $("fap-key").textContent = mac ? "⇧⌘N" : "Ctrl+Shift+N";

  const segmented = (id, field) => {
    $(id).addEventListener("click", (event) => {
      const button = event.target.closest("button");
      if (button) update({ [field]: button.dataset.value });
    });
  };
  const setSegment = (id, value) => {
    for (const button of $(id).querySelectorAll("button")) {
      const on = button.dataset.value === value;
      button.classList.toggle("on", on);
      button.setAttribute("aria-checked", on);
    }
  };

  segmented("appearance", "appearance");
  segmented("dock", "devtools_dock");

  const search = $("search");
  const custom = $("custom-search");
  search.addEventListener("change", () => {
    $("custom-search-row").hidden = search.value !== "custom";
    if (search.value === "custom") custom.focus();
    else update({ search_url: search.value });
  });
  custom.addEventListener("change", () => {
    if (custom.value.includes("%s")) update({ search_url: custom.value.trim() });
  });

  $("archive").addEventListener("change", (e) => update({ archive_after_hours: Number(e.target.value) }));
  $("agent").addEventListener("click", (e) => update({ agent_access: e.target.getAttribute("aria-checked") !== "true" }));
  $("history").addEventListener("click", () => circle.action("show_history"));
  $("clear").addEventListener("click", () => circle.action("clear_browsing_data"));
  $("report-bug").addEventListener("click", () => circle.action("report_bug"));
  $("check-updates").addEventListener("click", () => circle.action("check_for_updates"));
  $("auto-update").addEventListener("click", (e) => update({ auto_update: e.target.getAttribute("aria-checked") !== "true" }));
  $("spotlight-blur").addEventListener("click", (e) => update({ spotlight_blur: e.target.getAttribute("aria-checked") !== "true" }));
  $("grants").addEventListener("click", (event) => {
    const button = event.target.closest("[data-forget]");
    if (button) update({ forget_grant: button.dataset.forget });
  });

  const shortcuts = [
    ["Spotlight: tabs, sites and search", `${mod}K`],
    ["New tab", `${mod}T`],
    ["Edit address", `${mod}L`],
    ["Show or hide the sidebar", `${mod}S`],
    ["Reopen closed tab", `${mod}⇧T`],
    ["History", mac ? "⌘Y" : "Ctrl+H"],
    ["Clear browsing data", `${mod}⇧⌫`],
    ["Developer tools", mac ? "⌥⌘I" : "F12"],
    ["Settings", `${mod},`],
  ];
  $("shortcuts").innerHTML = shortcuts
    .map(([label, keys]) => `<div class="row"><div class="label">${circle.esc(label)}</div><div class="value"><kbd>${circle.esc(keys)}</kbd></div></div>`)
    .join("");

  function updateText(u) {
    const st = u?.status;
    if (u?.checking) return "Checking for updates…";
    switch (st?.status) {
      case "up_to_date": return "You're up to date.";
      case "downloading": return `Downloading ${st.version}… ${st.percent}%`;
      case "ready": return `Version ${st.version} is ready. It installs when Circle quits or restarts.`;
      case "available": return `Version ${st.version} is available to download.`;
      case "failed": return `Couldn't update: ${st.error}`;
      default: return "Updates are checked every few hours.";
    }
  }

  circle.render = (s) => {
    if (s.hue !== undefined) circle.setHue(s.hue);
    $("version").textContent = s.version;
    $("nav-version").textContent = `Circle ${s.version}`;
    $("spotlight-blur").setAttribute("aria-checked", s.spotlight_blur !== false);
    $("update-status").textContent = updateText(s.update);
    $("auto-update").setAttribute("aria-checked", s.auto_update);
    setSegment("appearance", s.appearance);
    setSegment("dock", s.devtools_dock);

    const known = [...search.options].some((o) => o.value === s.search_url);
    if (document.activeElement !== custom) {
      search.value = known ? s.search_url : "custom";
      $("custom-search-row").hidden = known;
      if (!known) custom.value = s.search_url;
    }

    const archive = $("archive");
    if (![...archive.options].some((o) => o.value === String(s.archive_after_hours))) {
      archive.add(new Option(`${s.archive_after_hours} hours`, s.archive_after_hours), 0);
    }
    archive.value = String(s.archive_after_hours);

    $("agent").setAttribute("aria-checked", s.agent_access);

    $("grants").innerHTML = s.grants.length
      ? s.grants
          .map((key) => {
            const [kind, ...rest] = key.split(":");
            const origin = rest.join(":");
            const what = kind === "media" ? "Camera and microphone" : "Notifications, location and other requests";
            return `<div class="row grant"><div class="label">${circle.esc(circle.host(origin) || origin)}<small>${what}</small></div>
              <div class="value"><button class="button" data-forget="${circle.esc(key)}">Forget</button></div></div>`;
          })
          .join("")
      : `<div class="empty">Sites you allow to use your camera, microphone or location will appear here.</div>`;

    if (findInput.value) filter();
    $("about").textContent = `Circle ${s.version}${s.profile_dir ? ` · Profile: ${s.profile_dir}` : ""}`;
  };

  circle.send({ cmd: "settings_ready" });
})();
