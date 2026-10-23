// The history page: every visit grouped by day, searchable, with per-entry
// removal and "Clear browsing data".
(() => {
  const list = document.getElementById("list");
  const more = document.getElementById("more");
  const q = document.getElementById("q");
  const dialog = document.getElementById("clear");
  const PAGE = 200;

  let visits = [];
  let exhausted = false;
  let loading = false;
  let generation = 0;

  const dayLabel = (date) => {
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const day = new Date(date);
    day.setHours(0, 0, 0, 0);
    const diff = Math.round((today - day) / 86400000);
    if (diff === 0) return "Today";
    if (diff === 1) return "Yesterday";
    return date.toLocaleDateString(undefined, {
      weekday: "long",
      month: "long",
      day: "numeric",
      year: day.getFullYear() === today.getFullYear() ? undefined : "numeric",
    });
  };

  const removeIcon = '<svg viewBox="0 0 12 12"><path d="m3 3 6 6M9 3 3 9" /></svg>';

  const render = () => {
    if (!visits.length) {
      list.innerHTML = `<div class="empty">${q.value.trim() ? "No matching pages" : "Pages you visit will show up here"}</div>`;
      return;
    }
    let html = "";
    let lastDay = "";
    for (const v of visits) {
      const date = new Date(v.at * 1000);
      const day = dayLabel(date);
      if (day !== lastDay) {
        html += `<h2>${circle.esc(day)}</h2>`;
        lastDay = day;
      }
      const time = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
      const title = v.title || circle.host(v.url) || v.url;
      html += `<a class="visit" href="${circle.esc(v.url)}" title="${circle.esc(v.url)}">
        <time>${circle.esc(time)}</time>
        ${circle.icon(v)}
        <span class="title ellipsis">${circle.esc(title)}</span>
        <span class="host ellipsis">${circle.esc(circle.host(v.url))}</span>
        <button class="remove" data-url="${circle.esc(v.url)}" data-at="${v.at}" title="Remove from history" aria-label="Remove from history">${removeIcon}</button>
      </a>`;
    }
    list.innerHTML = html;
  };

  const load = async (reset) => {
    if (reset) {
      generation++;
      visits = [];
      exhausted = false;
      loading = false;
    }
    if (loading || exhausted) return;
    loading = true;
    const mine = generation;
    const params = new URLSearchParams({ q: q.value.trim(), limit: PAGE });
    if (visits.length) params.set("before", visits[visits.length - 1].at);
    try {
      const res = await fetch(`circle://ui/api/history?${params}`);
      const { visits: page } = await res.json();
      if (mine !== generation) return;
      visits = visits.concat(page);
      exhausted = page.length < PAGE;
    } catch {
      exhausted = true;
    }
    loading = false;
    render();
    // Keep loading while the sentinel is still on screen.
    if (!exhausted && more.getBoundingClientRect().top < innerHeight + 400) load(false);
  };

  new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) load(false);
  }, { rootMargin: "400px" }).observe(more);

  let searchTimer = null;
  q.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => load(true), 120);
  });

  list.addEventListener("click", (event) => {
    const button = event.target.closest(".remove");
    if (!button) return;
    event.preventDefault();
    const at = Number(button.dataset.at);
    const url = button.dataset.url;
    circle.send({ cmd: "delete_history", url, at });
    visits = visits.filter((v) => !(v.url === url && v.at === at));
    render();
  });

  // ----- clear browsing data -------------------------------------------------

  document.getElementById("open-clear").addEventListener("click", () => dialog.showModal());

  dialog.addEventListener("close", async () => {
    if (dialog.returnValue !== "clear") return;
    const range = document.getElementById("range").value;
    await circle.send({
      cmd: "clear_browsing_data",
      hours: range === "all" ? null : Number(range),
      history: document.getElementById("c-history").checked,
      cookies: document.getElementById("c-cookies").checked,
      cache: document.getElementById("c-cache").checked,
    });
    // The command runs on the browser's UI thread; give it a moment.
    setTimeout(() => load(true), 150);
  });

  if (new URLSearchParams(location.search).has("clear")) dialog.showModal();
  load(true);
})();
