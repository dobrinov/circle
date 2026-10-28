// Circle's bridge: learns this page's Chrome tab id and tells Circle through
// the console of this isolated world, which the page can't see.
chrome.runtime.sendMessage("tab", (id) => {
  void chrome.runtime.lastError;
  if (typeof id === "number") console.debug("__circle_tab__ " + id);
});
