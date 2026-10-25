(() => {
  const $ = (id) => document.getElementById(id);
  let mac = navigator.platform.startsWith("Mac");

  const GREETINGS = {
    night: ["Still up?", "Hello, night owl", "Burning the midnight oil", "The quiet hours"],
    morning: ["Good morning", "Rise and shine", "Morning, sunshine", "Hello, early bird", "A fresh start"],
    afternoon: ["Good afternoon", "Hey there", "Afternoon!", "Hello again", "Keep it rolling"],
    evening: ["Good evening", "Evening!", "Winding down?", "Hello, evening", "Golden hour"],
  };
  const MOTIVATION = [
    "Small steps, every day.",
    "Stay curious.",
    "One tab at a time.",
    "Done is better than perfect.",
    "Focus on what matters.",
    "Less noise, more signal.",
    "Make it simple, then make it great.",
    "Go make something you're proud of.",
    "Today is a good day to learn something.",
    "You've got this.",
    "Curiosity is a superpower.",
    "Start where you are.",
    "Progress, not perfection.",
    "Ship it.",
    "Deep work beats busy work.",
    "Ask better questions.",
  ];
  const pick = (list) => list[Math.floor(Math.random() * list.length)];
  let picked = null;

  // A new greeting and line each time the splash appears, not on
  // every state update.
  function choose() {
    const h = new Date().getHours();
    const part = h < 5 ? "night" : h < 12 ? "morning" : h < 18 ? "afternoon" : h < 23 ? "evening" : "night";
    picked = { greeting: pick(GREETINGS[part]), lead: pick(MOTIVATION) };
  }

  function greet(isPrivate) {
    if (!picked) choose();
    $("greeting").textContent = isPrivate ? "Fap mode" : picked.greeting;
    $("lead").textContent = isPrivate ? "Nobody's watching. Nothing is saved." : picked.lead;
  }
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      choose();
      greet(document.body.classList.contains("private"));
    }
  });

  let isPrivate = false;

  function keys() {
    const mod = mac ? "⌘" : "Ctrl";
    $("keys").innerHTML = `<kbd>${mod}</kbd><kbd>K</kbd>`;
    const hint = (combo, label) => `<span>${combo.map((k) => `<kbd>${k}</kbd>`).join("")} ${label}</span>`;
    // Shortcuts people use every day; fap mode only shows inside it.
    const shift = (key) => (mac ? ["⇧", mod, key] : [mod, "Shift", key]);
    $("hints").innerHTML = (isPrivate
      ? [hint([mod, "T"], "New tab"), hint(shift("."), "End fap mode instantly")]
      : [hint([mod, "T"], "New tab"), hint([mod, "L"], "Edit address"), hint(shift("T"), "Reopen closed tab")]
    ).join("");
  }

  circle.render = (state) => {
    mac = state.platform === "macos";
    const space = state.spaces[state.active_space];
    circle.setHue(space.private ? 268 : space.hue);
    isPrivate = !!space.private;
    document.body.classList.toggle("private", isPrivate);
    greet(!!space.private);
    keys();
  };

  $("spotlight").addEventListener("click", () => circle.action({ open_palette: "spotlight" }));
  greet(false);
  keys();
  circle.send({ cmd: "ready", from: "start" });
})();
