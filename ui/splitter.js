(() => {
  // Dragging reports the distance moved since the press; Circle resizes
  // DevTools from that (and moves this overlay along with the gap).
  const body = document.body;
  let start = null;
  const grip = (phase, dx = 0, dy = 0) => circle.send({ cmd: "devtools_grip", phase, dx, dy });
  const fit = () => {
    const col = innerWidth < innerHeight;
    body.className = (col ? "col" : "row") + (start ? " dragging" : "");
    document.documentElement.style.cursor = col ? "col-resize" : "row-resize";
  };
  addEventListener("resize", fit);
  fit();
  document.documentElement.addEventListener("pointerdown", (e) => {
    start = [e.screenX, e.screenY];
    document.documentElement.setPointerCapture(e.pointerId);
    e.preventDefault();
    fit();
    grip("start");
  });
  document.documentElement.addEventListener("pointermove", (e) => {
    if (start) grip("move", Math.round(e.screenX - start[0]), Math.round(e.screenY - start[1]));
  });
  const end = () => {
    if (start) grip("end");
    start = null;
    fit();
  };
  document.documentElement.addEventListener("pointerup", end);
  document.documentElement.addEventListener("pointercancel", end);
})();
