// Drawn into a page while Circle records it, so the recording shows what the
// screencast can't: the pointer, clicks and the keys being pressed. Circle
// runs it (with `__OPTIONS__` filled in) in an isolated world, so the page's
// own scripts neither see it nor break it. The marks live in a closed shadow
// root that ignores the pointer, and go away when the recording stops.
(() => {
  const ID = "__circle_recording_overlay";
  if (!document.documentElement || document.getElementById(ID)) return;
  const options = __OPTIONS__;
  const mac = navigator.platform.startsWith("Mac");

  const host = document.createElement("div");
  host.id = ID;
  host.style.cssText = "all: initial; position: fixed; inset: 0; z-index: 2147483647; pointer-events: none;";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>
    :host { all: initial; }
    .pointer {
      position: fixed; left: 0; top: 0; width: 22px; height: 22px; margin: -11px 0 0 -11px;
      border-radius: 50%; background: hsl(268 90% 60% / 0.28); box-shadow: 0 0 0 1.5px hsl(268 90% 60% / 0.7);
      opacity: 0; transition: opacity 0.2s, transform 0.12s; will-change: transform;
    }
    .pointer.on { opacity: 1; }
    .pointer.down { transform: scale(0.8); background: hsl(268 90% 60% / 0.45); }
    .ripple {
      position: fixed; width: 44px; height: 44px; margin: -22px 0 0 -22px; border-radius: 50%;
      border: 2.5px solid hsl(268 90% 60% / 0.85); animation: ripple 0.5s ease-out forwards;
    }
    @keyframes ripple { from { transform: scale(0.3); opacity: 1; } to { transform: scale(1.4); opacity: 0; } }
    .keys {
      /* Above where Circle's floating recording controls sit. */
      position: fixed; left: 50%; bottom: 96px; transform: translateX(-50%);
      display: flex; gap: 6px; align-items: center; max-width: 80vw;
      padding: 10px 16px; border-radius: 14px; background: hsl(260 20% 12% / 0.82);
      color: white; font: 600 22px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
      box-shadow: 0 8px 30px hsl(0 0% 0% / 0.3); opacity: 0; transition: opacity 0.25s;
      white-space: pre; overflow: hidden;
    }
    .keys.on { opacity: 1; }
    .keys kbd {
      font: inherit; min-width: 1.2em; padding: 2px 8px; border-radius: 7px; text-align: center;
      background: hsl(0 0% 100% / 0.14); box-shadow: inset 0 -2px 0 hsl(0 0% 0% / 0.3);
    }
  </style><div class="pointer"></div><div class="keys"></div>`;
  const pointer = root.querySelector(".pointer");
  const keys = root.querySelector(".keys");
  document.documentElement.append(host);

  const live = () => host.isConnected;

  if (options.clicks) {
    addEventListener("pointermove", (e) => {
      if (!live()) return;
      pointer.style.transform = `translate(${e.clientX}px, ${e.clientY}px)${pointer.classList.contains("down") ? " scale(0.8)" : ""}`;
      pointer.classList.add("on");
    }, true);
    addEventListener("pointerdown", (e) => {
      if (!live()) return;
      pointer.classList.add("down");
      const ripple = document.createElement("div");
      ripple.className = "ripple";
      ripple.style.left = `${e.clientX}px`;
      ripple.style.top = `${e.clientY}px`;
      root.append(ripple);
      ripple.addEventListener("animationend", () => ripple.remove());
    }, true);
    addEventListener("pointerup", () => pointer.classList.remove("down"), true);
    document.addEventListener("mouseleave", () => pointer.classList.remove("on"));
  }

  if (options.keys) {
    const NAMES = {
      " ": "Space", Enter: "↩", Backspace: "⌫", Delete: "⌦", Tab: "⇥", Escape: "esc",
      ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
      PageUp: "⇞", PageDown: "⇟", Home: "↖", End: "↘",
    };
    const MODS = mac
      ? [["ctrlKey", "⌃"], ["altKey", "⌥"], ["shiftKey", "⇧"], ["metaKey", "⌘"]]
      : [["ctrlKey", "Ctrl"], ["altKey", "Alt"], ["shiftKey", "Shift"], ["metaKey", "Win"]];
    const secret = (el) =>
      el instanceof HTMLInputElement && (el.type === "password" || /cc-|one-time-code/.test(el.autocomplete || ""));
    let typed = "";
    let typedIn = null;
    let hide = null;
    const show = (html) => {
      keys.innerHTML = html;
      keys.classList.add("on");
      clearTimeout(hide);
      hide = setTimeout(() => {
        keys.classList.remove("on");
        typed = "";
      }, 1600);
    };
    const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
    addEventListener("keydown", (e) => {
      if (!live() || ["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn"].includes(e.key)) return;
      const target = e.composedPath()[0];
      const hidden = secret(target);
      // Typing in another field starts a new line.
      if (target !== typedIn) typed = "";
      typedIn = target;
      const mods = MODS.filter(([k]) => e[k]).map(([, s]) => s);
      const plain = e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey;
      if (plain) {
        // Typing accumulates into a line of text (dots for secrets).
        typed = (typed + (hidden ? "•" : e.key)).slice(-40);
        show(`<span>${esc(typed)}</span>`);
        return;
      }
      typed = "";
      const name = NAMES[e.key] || (e.key.length === 1 ? e.key.toUpperCase() : e.key);
      const shown = hidden && e.key.length === 1 ? "•" : name;
      show([...mods, shown].map((k) => `<kbd>${esc(k)}</kbd>`).join(""));
    }, true);
  }
})();
