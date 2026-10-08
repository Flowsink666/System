const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

// The offset is captured before an action, so its assertion cannot match an
// identical state left in the log by an earlier action.
async function waitForNewMarker(read, text, since, timeout = 30000, interval = 100) {
  const deadline = Date.now() + timeout;
  do {
    if (read().slice(since).includes(text)) return;
    await delay(interval);
  } while (Date.now() < deadline);
  throw new Error(`Missing new serial marker: ${text}\n${read().slice(since).slice(-2000)}`);
}

module.exports = { waitForNewMarker };
