(() => {
  const bar = document.getElementById("bar");
  const X = '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 3 6 6M9 3 3 9"/></svg>';
  let format = "video";
  let shown = "";
  let timer = null;

  circle.render = (state) => {
    const rec = state.recording;
    const prompt = !rec && state.record_prompt;
    clearInterval(timer);
    timer = null;
    bar.className = "";
    if (prompt) {
      if (shown !== "prompt") format = prompt.format;
      shown = "prompt";
      const mark = (name, label) =>
        `<button class="chip" data-mark="${name}" aria-pressed="${!!prompt[name]}" title="Show ${label.toLowerCase()} in the recording">${label}</button>`;
      bar.innerHTML = `<span class="dot"></span><b>Record this tab</b>
        <div class="seg" role="radiogroup" aria-label="Format">
          <button role="radio" data-format="video" class="${format === "video" ? "on" : ""}" aria-checked="${format === "video"}">Video</button>
          <button role="radio" data-format="gif" class="${format === "gif" ? "on" : ""}" aria-checked="${format === "gif"}">GIF</button>
        </div>
        <span class="sep"></span>${mark("clicks", "Clicks")}${mark("keys", "Keys")}<span class="sep"></span>
        <button class="start" data-start><span class="dot"></span>Start</button>
        <button class="icon" data-cancel title="Cancel (esc)" aria-label="Cancel">${X}</button>`;
    } else if (rec && rec.saving) {
      shown = "saving";
      bar.className = "saving";
      bar.innerHTML = `<span class="spinner"></span><b>Saving ${rec.format === "gif" ? "GIF" : "video"}…</b>`;
    } else if (rec) {
      shown = "recording";
      bar.className = "live";
      bar.innerHTML = `<span class="dot"></span><b>${rec.format === "gif" ? "Recording GIF" : "Recording"}</b> <time></time>
        <button data-stop title="Stop and save to Downloads">Stop</button>`;
      const tick = () => {
        const s = Math.max(0, Math.floor((Date.now() - rec.since) / 1000));
        bar.querySelector("time").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
      };
      tick();
      timer = setInterval(tick, 500);
    } else {
      shown = "";
      bar.innerHTML = "";
    }
    report();
  };

  let last = "";
  function report() {
    const r = bar.getBoundingClientRect();
    const size = `${Math.ceil(r.width)}x${Math.ceil(r.height)}`;
    if (r.width && size !== last) {
      last = size;
      circle.send({ cmd: "recbar_size", width: Math.ceil(r.width), height: Math.ceil(r.height) });
    }
  }
  new ResizeObserver(report).observe(bar);

  bar.addEventListener("click", (event) => {
    const t = event.target;
    if (t.closest("[data-stop]")) return circle.action("stop_recording");
    if (t.closest("[data-cancel]")) return circle.action("cancel_recording");
    if (t.closest("[data-start]")) return circle.action({ start_recording: format });
    const f = t.closest("[data-format]");
    if (f) {
      format = f.dataset.format;
      for (const b of bar.querySelectorAll("[data-format]")) {
        b.classList.toggle("on", b === f);
        b.setAttribute("aria-checked", b === f);
      }
      return;
    }
    const m = t.closest("[data-mark]");
    if (m) circle.send({ cmd: "settings", [`recording_${m.dataset.mark}`]: m.getAttribute("aria-pressed") !== "true" });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && shown === "prompt") circle.action("cancel_recording");
  });

  circle.send({ cmd: "ready", from: "recbar" });
})();
