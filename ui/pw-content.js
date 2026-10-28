// Circle's password helper, run in an isolated world of each web page: the
// page's own scripts can't see it or call it. It notices login fields and
// sign-ins and queues them; a console ping (with no data) tells Circle to
// collect the queue with `__circlePw.take()`. Circle fills a login with
// `__circlePw.fill()` after you pick it from its own suggestions.
(() => {
  if (window.__circlePw) return;
  const queue = [];
  let pinged = false;
  const post = (message) => {
    queue.push({ ...message, origin: location.origin });
    if (!pinged) {
      pinged = true;
      queueMicrotask(() => {
        pinged = false;
        console.debug("__circle_pw__");
      });
    }
  };

  const TEXTY = new Set(["text", "email", "tel", "url", ""]);
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden";
  };
  const isPassword = (el) => el instanceof HTMLInputElement && el.type === "password";
  const isTexty = (el) => el instanceof HTMLInputElement && TEXTY.has(el.type) && !el.disabled && !el.readOnly;
  const looksLikeUsername = (el) =>
    /username|email/.test(el.autocomplete || "") ||
    el.type === "email" ||
    /user|login|email|account|identifier/i.test(`${el.name} ${el.id} ${el.getAttribute("aria-label") || ""}`);

  const inputsOf = (scope) => [...(scope || document).querySelectorAll("input")].filter(visible);
  const passwordOf = (el) => {
    const scope = el.form || document;
    return inputsOf(scope).find(isPassword) || null;
  };
  // The username that goes with a password field: one marked as such, or
  // the last text field before it.
  const usernameOf = (password) => {
    const fields = inputsOf(password?.form || document).filter(isTexty);
    const marked = fields.find((f) => /username|email/.test(f.autocomplete || ""));
    if (marked) return marked;
    if (!password) return fields.find(looksLikeUsername) || null;
    const before = fields.filter((f) => f.compareDocumentPosition(password) & Node.DOCUMENT_POSITION_FOLLOWING);
    return before[before.length - 1] || null;
  };

  // A login field: a password, or a username next to one (or alone, on the
  // first page of a two-step sign-in).
  const loginField = (el) => {
    if (isPassword(el) && visible(el)) return "password";
    if (isTexty(el) && visible(el)) {
      const password = passwordOf(el);
      if (password && usernameOf(password) === el) return "username";
      if (!password && looksLikeUsername(el) && el.autocomplete !== "off") return "username";
    }
    return null;
  };

  let current = null;
  let lastUsername = "";
  const rectOf = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  };
  document.addEventListener(
    "focusin",
    (event) => {
      const field = loginField(event.target);
      if (!field) return;
      current = event.target;
      post({ type: "focus", field, rect: rectOf(current) });
    },
    true,
  );
  document.addEventListener(
    "focusout",
    () => {
      // Focus going to Circle's suggestions (outside the page) isn't a blur.
      setTimeout(() => {
        if (document.hasFocus() && current && document.activeElement !== current) {
          current = null;
          post({ type: "blur" });
        }
      });
    },
    true,
  );
  let moved = false;
  const hide = () => {
    if (!current || moved) return;
    moved = true;
    requestAnimationFrame(() => (moved = false));
    post({ type: "blur" });
  };
  addEventListener("scroll", hide, true);
  addEventListener("resize", hide);

  document.addEventListener(
    "input",
    (event) => {
      const el = event.target;
      if (isTexty(el) && loginField(el) === "username") lastUsername = el.value;
      // Typing narrows nothing yet; hide the suggestions once you type.
      if (el === current && event.isTrusted && event.inputType) post({ type: "typing" });
    },
    true,
  );

  // A sign-in: a form with a filled password being submitted, by the form
  // itself, Enter, or a click on its button.
  let lastSent = "";
  const signIn = (scope) => {
    const password = inputsOf(scope).find((f) => isPassword(f) && f.value);
    if (!password) return;
    const user = usernameOf(password);
    const username = (user && user.value) || lastUsername;
    const key = `${username}\n${password.value}`;
    if (key === lastSent) return;
    lastSent = key;
    post({ type: "submit", username, password: password.value });
  };
  document.addEventListener("submit", (event) => signIn(event.target), true);
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Enter" && isPassword(event.target)) signIn(event.target.form || document);
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      const button = event.target.closest?.("button, input[type=submit], [role=button]");
      if (!button) return;
      const scope = button.form || button.closest("form") || document;
      if (inputsOf(scope).some(isPassword)) signIn(scope);
    },
    true,
  );
  addEventListener("pagehide", () => signIn(document), true);

  // Sets a field the way typing would, so the page's own code notices.
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  const put = (el, value) => {
    if (!el) return;
    el.focus();
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  };

  window.__circlePw = {
    take: () => JSON.stringify(queue.splice(0)),
    fill(username, password) {
      const anchor = current && document.contains(current) ? current : document.activeElement;
      const pw = isPassword(anchor) ? anchor : passwordOf(anchor || document.body);
      const user = pw ? usernameOf(pw) : isTexty(anchor) ? anchor : usernameOf(null);
      if (user && username) put(user, username);
      if (pw) put(pw, password);
      lastUsername = username;
      current = null;
    },
  };

  // A field that already had focus when the page finished loading.
  const active = document.activeElement;
  const field = active && loginField(active);
  if (field) {
    current = active;
    post({ type: "focus", field, rect: rectOf(active) });
  }
})();
