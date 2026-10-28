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
  for (const name of ["clicks", "keys", "float"]) {
    $(`recording-${name}`).addEventListener("click", (e) =>
      update({ [`recording_${name}`]: e.target.getAttribute("aria-checked") !== "true" }),
    );
  }
  // ----- passwords ----------------------------------------------------------------

  let logins = [];
  let shown = {}; // id → password, while revealed
  let copied = null;
  let deleting = null; // asked once; a second click deletes
  const EYE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/></svg>';
  const COPY = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M11 5V3.5A1.5 1.5 0 0 0 9.5 2h-6A1.5 1.5 0 0 0 2 3.5v6A1.5 1.5 0 0 0 3.5 11H5"/></svg>';
  const TRASH = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 4h11M6 4V2.5h4V4M4 4l.7 9.5h6.6L12 4"/></svg>';
  const drawLogins = () => {
    const query = $("pw-search").value.trim().toLowerCase();
    const list = logins.filter((l) => !query || `${l.origin} ${l.username}`.toLowerCase().includes(query));
    $("pw-search").hidden = logins.length < 4;
    $("logins").innerHTML = list.length
      ? list
          .map((l) => {
            const host = circle.host(l.origin) || l.origin;
            const secret = shown[l.id] !== undefined ? `<span class="secret">${circle.esc(shown[l.id])}</span>` : "••••••••";
            return `<div class="row login"><div class="label"><div class="site">${circle.icon({ url: l.origin, live: false }, 16)}<span>${circle.esc(host)}</span></div>
                <small>${circle.esc(l.username || "(no username)")} · ${secret}</small></div>
              <div class="value">${copied === l.id ? '<span class="copied">Copied</span>' : ""}
                <button class="button icon-btn" data-pw-show="${l.id}" title="${shown[l.id] !== undefined ? "Hide" : "Show"} password" aria-label="Show password">${EYE}</button>
                <button class="button icon-btn" data-pw-copy="${l.id}" title="Copy password" aria-label="Copy password">${COPY}</button>
                ${deleting === l.id
                  ? `<button class="button danger" data-pw-delete="${l.id}">Delete</button>`
                  : `<button class="button icon-btn danger" data-pw-delete="${l.id}" title="Delete" aria-label="Delete">${TRASH}</button>`}</div></div>`;
          })
          .join("")
      : `<div class="empty">${logins.length ? "No saved passwords match." : "Passwords you save when signing in will appear here."}</div>`;
  };
  $("pw-search").addEventListener("input", drawLogins);
  $("logins").addEventListener("click", (event) => {
    const show = event.target.closest("[data-pw-show]");
    if (show) {
      const id = Number(show.dataset.pwShow);
      if (shown[id] !== undefined) {
        delete shown[id];
        return drawLogins();
      }
      return circle.send({ cmd: "password_reveal", id });
    }
    const copy = event.target.closest("[data-pw-copy]");
    if (copy) return circle.send({ cmd: "password_copy", id: Number(copy.dataset.pwCopy) });
    const del = event.target.closest("[data-pw-delete]");
    if (del) {
      const id = Number(del.dataset.pwDelete);
      if (deleting === id) {
        deleting = null;
        delete shown[id];
        circle.send({ cmd: "password_delete", id });
      } else {
        deleting = id;
        drawLogins();
      }
    }
  });
  $("never").addEventListener("click", (event) => {
    const forget = event.target.closest("[data-forget-never]");
    if (forget) circle.send({ cmd: "password_forget_never", origin: forget.dataset.forgetNever });
  });
  $("passwords-enabled").addEventListener("click", (e) => update({ passwords_enabled: e.target.getAttribute("aria-checked") !== "true" }));
  $("passwords-offer").addEventListener("click", (e) => update({ passwords_offer: e.target.getAttribute("aria-checked") !== "true" }));

  // ----- extensions -----------------------------------------------------------------

  let extensions = [];
  let removing = null; // asked once; a second click removes
  const PIN = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.5 2.5 13.5 6.5 11 7.5 8.5 10l.5 3-2-2-3.5 3.5M7 11 5 9m3.5 1L6 7.5 9 5l.5-2.5"/></svg>';
  const GEAR = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="2"/><path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M3.6 12.4l1.2-1.2M11.2 4.8l1.2-1.2"/></svg>';
  const drawExtensions = () => {
    const query = $("ext-search").value.trim().toLowerCase();
    const list = extensions.filter((e) => !query || `${e.name} ${e.description}`.toLowerCase().includes(query));
    $("ext-search").hidden = extensions.length < 5;
    $("ext-list").innerHTML = list.length
      ? list
          .map((e) => {
            const status = e.error
              ? `<small class="error">${circle.esc(e.error)}</small>`
              : e.restart
                ? `<small class="status">${e.enabled ? "Updated; the new version starts after a restart" : "Stops when Circle restarts"}</small>`
                : "";
            const source = e.from_store ? "" : `<small>Unpacked · ${circle.esc(e.path)}</small>`;
            const perms = e.warnings.length
              ? `<details><summary>What it can do</summary><ul>${e.warnings.map((w) => `<li>${circle.esc(w)}</li>`).join("")}</ul></details>`
              : "";
            return `<div class="row ext${e.enabled ? "" : " off"}">
                <img class="ext-icon" src="circle://ui/ext-icon/${circle.esc(e.id)}" alt="">
                <div class="label"><b>${circle.esc(e.name)}</b><span class="version">${circle.esc(e.version)}</span>
                  ${e.description ? `<small class="desc">${circle.esc(e.description)}</small>` : ""}${source}${status}${perms}</div>
                <div class="value">
                  ${e.enabled ? `<button class="button icon-btn${e.pinned ? " on" : ""}" data-ext-pin="${e.id}" title="${e.pinned ? "Unpin from" : "Pin to"} the sidebar" aria-pressed="${e.pinned}">${PIN}</button>` : ""}
                  ${e.options && e.running ? `<button class="button icon-btn" data-ext-options="${e.id}" title="Options">${GEAR}</button>` : ""}
                  ${removing === e.id
                    ? `<button class="button danger" data-ext-remove="${e.id}">Remove</button>`
                    : `<button class="button icon-btn danger" data-ext-remove="${e.id}" title="Remove" aria-label="Remove">${TRASH}</button>`}
                  <button class="switch" role="switch" data-ext-enable="${e.id}" aria-checked="${e.enabled}" aria-label="Turn ${circle.esc(e.name)} on or off"></button>
                </div></div>`;
          })
          .join("")
      : `<div class="empty">${extensions.length ? "No extensions match." : "No extensions yet. Open an extension's page in the Chrome Web Store and choose “Add to Circle”."}</div>`;
    // An extension without an icon gets a plain tile.
    for (const img of $("ext-list").querySelectorAll("img.ext-icon")) {
      img.addEventListener("error", () => {
        img.removeAttribute("src");
        img.classList.add("none");
      }, { once: true });
    }
  };
  $("ext-search").addEventListener("input", drawExtensions);
  $("ext-list").addEventListener("click", (event) => {
    const on = (name) => event.target.closest(`[data-ext-${name}]`)?.getAttribute(`data-ext-${name}`);
    let id;
    if ((id = on("enable"))) return circle.send({ cmd: "ext_enable", id, on: event.target.getAttribute("aria-checked") !== "true" });
    if ((id = on("pin"))) return circle.send({ cmd: "ext_pin", id, pinned: event.target.closest("[data-ext-pin]").getAttribute("aria-pressed") !== "true" });
    if ((id = on("options"))) return circle.send({ cmd: "ext_options", id });
    if ((id = on("remove"))) {
      if (removing === id) {
        removing = null;
        circle.send({ cmd: "ext_remove", id });
      } else {
        removing = id;
        drawExtensions();
      }
    }
  });
  $("ext-store").addEventListener("click", () => circle.send({ cmd: "ext_store" }));
  $("ext-unpacked").addEventListener("click", () => circle.send({ cmd: "ext_load_unpacked" }));
  $("ext-restart-now").addEventListener("click", () => circle.action("restart"));

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
    ["Duplicate tab", `${mod}⇧D`],
    ["Split view on or off (or drag a tab onto the page)", `${mod}⇧=`],
    ["History", mac ? "⌘Y" : "Ctrl+H"],
    ["Clear browsing data", `${mod}⇧⌫`],
    ["Developer tools", mac ? "⌥⌘I" : "F12"],
    ["Network", mac ? "⌥⌘E" : "Ctrl+Shift+E"],
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
    for (const name of ["clicks", "keys", "float"]) {
      $(`recording-${name}`).setAttribute("aria-checked", s[`recording_${name}`] !== false);
    }
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

    $("passwords-enabled").setAttribute("aria-checked", s.passwords_enabled !== false);
    $("passwords-offer").setAttribute("aria-checked", s.passwords_offer !== false);
    $("passwords-offer").disabled = s.passwords_enabled === false;
    $("passwords-error").hidden = !s.passwords_error;
    $("passwords-error").textContent = s.passwords_error ? `Saved passwords are unavailable: ${s.passwords_error}` : "";
    logins = s.passwords || [];
    if (s.revealed) shown[s.revealed.id] = s.revealed.password;
    if (s.copied !== undefined) {
      copied = s.copied;
      setTimeout(() => {
        copied = null;
        drawLogins();
      }, 2000);
    }
    drawLogins();
    const never = s.passwords_never || [];
    $("never").hidden = !never.length;
    $("never").innerHTML = never
      .map(
        (origin) => `<div class="row"><div class="label">${circle.esc(circle.host(origin) || origin)}<small>Never offered to save</small></div>
          <div class="value"><button class="button" data-forget-never="${circle.esc(origin)}">Allow again</button></div></div>`,
      )
      .join("");

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

    const ext = s.extensions || { list: [] };
    extensions = ext.list;
    $("ext-restart").hidden = !ext.restart;
    $("ext-unpacked").hidden = ext.available === false;
    $("ext-store").hidden = ext.available === false;
    drawExtensions();
    if (s.section && $(`${s.section}-section`)) showSection(`${s.section}-section`);

    if (findInput.value) filter();
    $("about").textContent = `Circle ${s.version}${s.profile_dir ? ` · Profile: ${s.profile_dir}` : ""}`;
  };

  circle.send({ cmd: "settings_ready" });
})();
