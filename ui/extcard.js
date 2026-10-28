// Circle's card on Chrome Web Store pages: the store's own button only works
// in Chrome, so this adds the extension to Circle instead.
(() => {
  const card = document.getElementById("card");
  const X = '<svg viewBox="0 0 10 10" aria-hidden="true"><path d="m1 1 8 8M9 1 1 9"/></svg>';
  const close = '<button class="close" data-close title="Close" aria-label="Close">' + X + "</button>";

  const fit = () =>
    requestAnimationFrame(() =>
      circle.send({ cmd: "ext_card_size", width: Math.ceil(card.offsetWidth), height: Math.ceil(card.offsetHeight) }),
    );

  const head = (s, title, sub, withIcon) => `<div class="title">
      ${withIcon ? `<img class="icon" src="circle://ui/ext-icon/${s.id}" alt="">` : '<span class="mark" aria-hidden="true"></span>'}
      <div><b>${title}</b><span>${sub}</span></div>${close}</div>`;

  circle.render = (s) => {
    if (s.hue !== undefined) circle.setHue(s.hue);
    const name = circle.esc(s.name || "this extension");
    switch (s.stage) {
      case "available":
        card.innerHTML = `${head(s, `Add “${name}” to Circle`, "From the Chrome Web Store")}
          <div class="body">The store's own button only works in Chrome. Circle can install it for you.</div>
          <div class="actions"><button class="btn primary" data-add>Add to Circle</button></div>`;
        break;
      case "fetching":
        card.innerHTML = `${head(s, `Adding “${name}”`, "From the Chrome Web Store")}
          <div class="body"><span class="spin"></span>Downloading and checking its signature…</div>`;
        break;
      case "confirm": {
        const warnings = s.warnings.length
          ? `<p>It will be able to:</p><ul>${s.warnings.map((w) => `<li>${circle.esc(w)}</li>`).join("")}</ul>`
          : "<p>It needs no special permissions.</p>";
        card.innerHTML = `${head(s, `Add “${name}”?`, `Version ${circle.esc(s.version)}`, true)}
          <div class="perms">${warnings}</div>
          <div class="actions"><button class="btn quiet" data-cancel>Cancel</button><button class="btn primary" data-confirm>Add extension</button></div>`;
        break;
      }
      case "added":
        card.innerHTML = `${head(s, `“${name}” is in Circle`, "Added", true)}
          <div class="body">${s.pinned ? "Its button is next to the address bar in the sidebar." : "Find it under the puzzle button next to the address bar."}</div>
          <div class="actions"><button class="btn quiet" data-manage>Manage</button><button class="btn primary" data-close>Done</button></div>`;
        break;
      case "installed":
        card.innerHTML = `${head(s, `“${name}” is in Circle`, s.enabled ? "Installed" : "Installed, turned off", true)}
          <div class="actions"><button class="btn quiet" data-manage>Manage extensions</button></div>`;
        break;
      case "failed":
        card.innerHTML = `${head(s, `Couldn't add “${name}”`, "From the Chrome Web Store")}
          <div class="body error">${circle.esc(s.error)}</div>
          <div class="actions"><button class="btn quiet" data-close>Close</button><button class="btn primary" data-add>Try again</button></div>`;
        break;
      default:
        card.innerHTML = "";
    }
    fit();
  };

  card.addEventListener("click", (event) => {
    const on = (attr) => event.target.closest(`[data-${attr}]`);
    if (on("add")) return circle.send({ cmd: "ext_add" });
    if (on("confirm")) return circle.send({ cmd: "ext_confirm" });
    if (on("cancel")) return circle.send({ cmd: "ext_cancel" });
    if (on("manage")) return circle.send({ cmd: "ext_manage" });
    if (on("close")) return circle.send({ cmd: "ext_card_close" });
  });
  card.addEventListener("load", fit, true);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") circle.send({ cmd: "ext_card_close" });
  });

  circle.send({ cmd: "ready", from: "extcard" });
})();
