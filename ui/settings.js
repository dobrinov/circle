// The settings page. Circle pushes the current values with
// `circle.render(settings)`; changes go back as `{cmd: "settings", ...}`.
(() => {
  const $ = (id) => document.getElementById(id);
  const mac = navigator.platform.startsWith("Mac");
  const mod = mac ? "⌘" : "Ctrl+";
  const update = (fields) => circle.send({ cmd: "settings", ...fields });

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

  circle.render = (s) => {
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

    $("about").textContent = `Circle ${s.version}${s.profile_dir ? ` · Profile: ${s.profile_dir}` : ""}`;
  };

  circle.send({ cmd: "settings_ready" });
})();
