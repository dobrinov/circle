// The drop card for split view: which half a dragged tab lands in.
(() => {
  let id = null;
  const zones = [...document.querySelectorAll("[data-zone]")];
  const clear = () => zones.forEach((z) => z.classList.remove("over"));
  const zoneAt = (event) => event.target.closest?.("[data-zone]");

  circle.render = (s) => {
    if (s.hue !== undefined) circle.setHue(s.hue);
    id = s.id ?? null;
    clear();
  };

  document.addEventListener("dragover", (event) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    const zone = zoneAt(event);
    zones.forEach((z) => z.classList.toggle("over", z === zone));
  });
  document.addEventListener("dragleave", (event) => {
    if (!event.relatedTarget) clear();
  });
  document.addEventListener("drop", (event) => {
    event.preventDefault();
    const zone = zoneAt(event)?.dataset.zone;
    clear();
    if (zone && id !== null) {
      circle.send({ cmd: "split", id, stacked: zone === "top" || zone === "bottom", first: zone === "left" || zone === "top" });
    }
    circle.send({ cmd: "tab_drag", id: null });
  });

  circle.send({ cmd: "ready", from: "splitdrop" });
})();
