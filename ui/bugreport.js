// The bug report page: an interview, a recording of what happens while the
// person reproduces the problem (kept by Circle, see src/bugreport.rs), and
// the resulting GitHub issue text. Circle pushes its state with
// circle.render({ recording, result, environment }).
(() => {
  const $ = (id) => document.getElementById(id);
  const FIELDS = ["title", "what", "expected", "steps", "frequency", "video"];
  const STORE = "circle-bug-report";

  let state = { recording: null, result: null, environment: null };
  let skipped = false;
  let edited = false;
  let ticker = null;

  // ----- answers survive reloads of this tab ---------------------------------

  const answers = () =>
    Object.fromEntries(FIELDS.map((f) => [f, $(f).type === "checkbox" ? $(f).checked : $(f).value.trim()]));
  const save = () => {
    try {
      sessionStorage.setItem(STORE, JSON.stringify({ answers: answers(), skipped }));
    } catch {}
  };
  try {
    const saved = JSON.parse(sessionStorage.getItem(STORE) || "null");
    if (saved) {
      for (const f of FIELDS) {
        if ($(f).type === "checkbox") $(f).checked = !!saved.answers[f];
        else $(f).value = saved.answers[f] ?? "";
      }
      skipped = !!saved.skipped;
    }
  } catch {}
  for (const f of FIELDS) $(f).addEventListener("input", save);

  // ----- views -----------------------------------------------------------------

  function show(view) {
    for (const id of ["interview", "recording", "report"]) $(id).hidden = id !== view;
    for (const step of document.querySelectorAll("[data-step]")) step.classList.toggle("on", step.dataset.step === view);
  }

  circle.render = (next) => {
    state = next;
    clearInterval(ticker);
    if (state.recording) {
      show("recording");
      const started = Date.now() - state.recording.elapsed;
      const tick = () => {
        const s = Math.floor((Date.now() - started) / 1000);
        $("elapsed").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
      };
      tick();
      ticker = setInterval(tick, 500);
    } else if (state.result || skipped) {
      show("report");
      if (!edited) $("report-title").value = $("title").value.trim();
      renderReport();
    } else {
      show("interview");
    }
  };

  $("start").addEventListener("click", () => {
    skipped = false;
    edited = false;
    save();
    circle.send({ cmd: "bug_report_start", video: $("video").checked });
  });
  $("skip").addEventListener("click", () => {
    skipped = true;
    save();
    circle.render({ ...state, result: null, recording: null });
  });
  $("stop").addEventListener("click", () => circle.send({ cmd: "bug_report_stop" }));
  $("again").addEventListener("click", () => {
    skipped = false;
    edited = false;
    state.result = null;
    save();
    show("interview");
  });
  for (const id of ["full-urls", "console"]) $(id).addEventListener("change", () => renderReport(true));
  $("body").addEventListener("input", () => (edited = true));

  // ----- the report --------------------------------------------------------------

  const clock = (ms) => {
    const s = Math.floor(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };

  // Full addresses can hold private paths and tokens; by default only the
  // site is kept.
  function place(url) {
    if (!url) return "";
    if ($("full-urls").checked) return url;
    try {
      const u = new URL(url);
      if (u.protocol === "http:" || u.protocol === "https:") return u.host;
      if (u.protocol === "file:") return "a local file";
      return `${u.protocol}//${u.host}`;
    } catch {
      return url;
    }
  }

  const ACTION_NAMES = {
    CloseTab: "Close tab", ReopenClosed: "Reopen closed tab", ToggleSidebar: "Toggle sidebar", TogglePin: "Pin/unpin tab",
    Reload: "Reload", ReloadHard: "Hard reload", Back: "Back", Forward: "Forward", NextTab: "Next tab", PrevTab: "Previous tab",
    ToggleSplit: "Split view", DevTools: "Developer tools", Fullscreen: "Full screen", NewSpace: "New space",
  };
  const actionName = (text) => ACTION_NAMES[text] || text.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/\(|\)/g, " ").trim();

  function timelineRow(e) {
    const times = e.count > 1 ? ` (×${e.count})` : "";
    switch (e.kind) {
      case "start":
      case "stop":
        return [e.text, ""];
      case "navigate":
        return ["Went to", place(e.url) + times];
      case "switch tab":
        return ["Switched to tab", `${e.text ? `“${e.text}” ` : ""}${place(e.url)}`];
      case "close tab":
        return ["Closed tab", place(e.url)];
      case "action":
        return ["Used", actionName(e.text) + times];
      case "load failed":
      case "frame load failed":
        return [e.kind === "load failed" ? "Page failed to load" : "Frame failed to load", `${e.text} · ${place(e.url)}`];
      case "page crashed":
        return ["Page crashed", `${e.text} · ${place(e.url)}`];
      default:
        return null;
    }
  }

  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const cell = (text) => String(text).replace(/\|/g, "\\|").replace(/\n/g, " ");

  function buildBody() {
    const a = answers();
    const env = state.environment || {};
    const r = state.result;
    const lines = [];
    const section = (heading, text) => {
      lines.push(`### ${heading}`, "", text || "_Not given._", "");
    };
    section("What happened", a.what);
    section("What I expected", a.expected);
    section("Steps to reproduce", a.steps || (r ? "_See the recording below._" : ""));
    lines.push(`**How often:** ${a.frequency}`, "");

    if (r) {
      const events = r.events || [];
      const rows = events.map((e) => [e, timelineRow(e)]).filter(([, row]) => row);
      lines.push("### What Circle recorded", "", `Recorded for ${clock(r.duration)}.`, "");
      lines.push("| Time | Event | Details |", "| --- | --- | --- |");
      const shown = rows.length > 60 ? [...rows.slice(0, 30), null, ...rows.slice(-29)] : rows;
      for (const item of shown) {
        if (!item) {
          lines.push(`| … | ${rows.length - 59} more | |`);
          continue;
        }
        const [e, [what, details]] = item;
        lines.push(`| ${clock(e.at)} | ${cell(what)} | ${cell(details)} |`);
      }
      lines.push("");
      const consoleEvents = events.filter((e) => e.kind.startsWith("console"));
      if ($("console").checked && consoleEvents.length) {
        const errors = consoleEvents.filter((e) => e.kind === "console error").length;
        lines.push(
          `<details><summary>Page errors and warnings (${plural(errors, "error")}, ${plural(consoleEvents.length - errors, "warning")})</summary>`,
          "",
          "```",
        );
        for (const e of consoleEvents.slice(-80)) {
          const level = e.kind === "console error" ? "error" : "warn ";
          // Console sources are "script-url:line".
          const source = e.url && e.url.match(/^(.*):(\d+)$/);
          const where = !source ? "" : $("full-urls").checked ? ` (${source[1]}:${source[2]})` : ` (${place(source[1])})`;
          lines.push(`[${clock(e.at)}] ${level} ${e.text}${where}${e.count > 1 ? ` ×${e.count}` : ""}`);
        }
        lines.push("```", "", "</details>", "");
      }
      if (r.dropped) lines.push(`_${r.dropped} older events were dropped._`, "");
    }

    lines.push("### Environment", "");
    lines.push(`- Circle ${env.circle || "?"} (Chromium ${env.chromium || "?"})`);
    lines.push(`- ${env.os || "?"}, ${env.arch || "?"}`);
    if (r?.video_file) lines.push("", `_A screen recording is attached: ${r.video_file}_`);
    return lines.join("\n").trim() + "\n";
  }

  function renderReport(force = false) {
    if (!edited || force) {
      $("body").value = buildBody();
      edited = false;
    }
    const r = state.result;
    const note = $("video-note");
    note.hidden = !r?.video;
    if (r?.video) {
      note.textContent = r.video_file
        ? `Your screen recording was saved to Downloads as “${r.video_file}”. Drag it into the GitHub issue to attach it.`
        : "Saving your screen recording to Downloads…";
    }
  }

  function status(text) {
    $("status").textContent = text;
    setTimeout(() => ($("status").textContent = ""), 4000);
  }

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      $("body").select();
      return document.execCommand("copy");
    }
  }

  $("copy").addEventListener("click", async () => {
    status((await copy($("body").value)) ? "Copied." : "Select the text and copy it.");
  });

  $("open-issue").addEventListener("click", async () => {
    const repo = state.environment?.repository || "https://github.com/dobrinov/circle";
    const title = $("report-title").value.trim() || "Bug report";
    // The report goes through the clipboard, not the address: an address
    // would keep it in history (and in GitHub's logs) even if never posted.
    const copied = await copy($("body").value);
    const body = copied
      ? "<!-- The report was copied to your clipboard: paste it here. -->\n"
      : "<!-- Copy the report from Circle and paste it here. -->\n";
    status(copied ? "Copied. Paste it into the issue." : "Copy the report and paste it into the issue.");
    circle.send({ cmd: "navigate", input: `${repo}/issues/new?${new URLSearchParams({ title, body })}`, new_tab: true });
  });

  circle.send({ cmd: "bug_report_ready" });
})();
