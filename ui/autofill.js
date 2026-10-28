// Circle's password overlay: saved logins for the focused field, or the
// prompt to save one after signing in. Passwords never reach this page.
(() => {
  const card = document.getElementById("card");
  const KEY = '<svg class="key" viewBox="0 0 16 16" aria-hidden="true"><circle cx="5.5" cy="10.5" r="3"/><path d="m7.6 8.4 6-6M11.5 4.5l1.8 1.8M9.8 6.2l1.4 1.4"/></svg>';
  const X = '<svg viewBox="0 0 10 10" aria-hidden="true"><path d="m1 1 8 8M9 1 1 9"/></svg>';

  const fit = () =>
    requestAnimationFrame(() =>
      circle.send({ cmd: "autofill_size", width: Math.ceil(card.offsetWidth), height: Math.ceil(card.offsetHeight) }),
    );

  circle.render = (s) => {
    if (s.hue !== undefined) circle.setHue(s.hue);
    card.className = s.mode;
    if (s.mode === "suggest") {
      card.innerHTML = `<div class="head">${KEY}<span>Saved for ${circle.esc(s.host)}</span></div>
        ${s.accounts
          .map(
            (a) => `<button class="account" data-pick="${a.id}">
              <span class="avatar" style="--letter-hue:${circle.hash(a.username || s.host)}">${circle.esc((a.username || "?")[0].toUpperCase())}</span>
              <span class="who">${circle.esc(a.username || "(no username)")}</span><span class="dots">••••••</span></button>`,
          )
          .join("")}
        <div class="foot"><button class="link" data-manage>Manage passwords…</button></div>`;
    } else if (s.mode === "save") {
      card.innerHTML = `<div class="title"><span class="mark" aria-hidden="true"></span>
          <div><b>${s.update ? "Update" : "Save"} password?</b><span>${circle.esc(s.host)}</span></div>
          <button class="close" data-close title="Not now" aria-label="Not now">${X}</button></div>
        <dl class="fields"><dt>Username</dt><dd>${circle.esc(s.username || "—")}</dd><dt>Password</dt><dd>••••••••</dd></dl>
        <div class="actions">
          ${s.update ? "" : '<button class="btn quiet" data-never>Never for this site</button>'}
          <button class="btn primary" data-save>${s.update ? "Update" : "Save"}</button>
        </div>`;
    } else {
      card.innerHTML = "";
    }
    fit();
  };

  card.addEventListener("click", (event) => {
    const pick = event.target.closest("[data-pick]");
    if (pick) return circle.send({ cmd: "autofill_pick", id: Number(pick.dataset.pick) });
    if (event.target.closest("[data-save]")) return circle.send({ cmd: "autofill_save" });
    if (event.target.closest("[data-never]")) return circle.send({ cmd: "autofill_never" });
    if (event.target.closest("[data-close]")) return circle.send({ cmd: "autofill_close" });
    if (event.target.closest("[data-manage]")) return circle.send({ cmd: "autofill_manage" });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") circle.send({ cmd: "autofill_close" });
  });

  circle.send({ cmd: "ready", from: "autofill" });
})();
