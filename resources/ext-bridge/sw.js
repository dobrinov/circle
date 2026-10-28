chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message === "tab" && sender.tab) reply(sender.tab.id);
});
