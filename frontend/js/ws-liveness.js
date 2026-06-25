// Den - WebSocket liveness (half-open detection)
//
// Browsers leave a WebSocket in readyState OPEN after the network drops (sleep,
// background, transport loss) without ever firing `close`. The terminal's
// reconnect logic is driven solely by `onclose`, so a half-open socket goes
// silent forever and only a fresh tab recovers it. This module isolates the
// pure decision — "does this connection look half-open?" — so it can be unit
// tested without a real socket. The caller pings periodically, records when the
// ping went out (`pingSentTs`) and when the last inbound frame arrived
// (`lastReceiveTs`), then asks `isStale` after a grace window. The server
// answers every ping with a `{"type":"pong"}` frame so an idle-but-alive
// connection still produces inbound traffic and is never falsely closed.
const DenWsLiveness = (() => {
  /**
   * @param {number} lastReceiveTs timestamp of the most recent inbound frame
   * @param {number} pingSentTs    timestamp of the last ping sent (0/falsy = none outstanding)
   * @param {number} graceMs       how long to wait for a response before declaring half-open
   * @param {number} now           current timestamp
   * @returns {boolean} true when the connection looks half-open and should be force-closed
   */
  function isStale(lastReceiveTs, pingSentTs, graceMs, now) {
    if (!pingSentTs) return false; // no ping outstanding → nothing to time out
    if (lastReceiveTs >= pingSentTs) return false; // a frame arrived since the ping → alive
    return now - pingSentTs >= graceMs; // grace elapsed in silence → half-open
  }

  /**
   * Whether a freshly sent ping should (re)start the liveness window. Restart
   * only when no prior ping is still unanswered; otherwise keep timing from the
   * oldest unanswered ping so a burst of pings (e.g. repeated resume events
   * firing closer together than the grace window) can't push the baseline
   * forward forever and indefinitely defer half-open detection.
   * @param {number} lastReceiveTs timestamp of the most recent inbound frame
   * @param {number} pingSentTs    timestamp of the current liveness baseline (0/falsy = none)
   * @returns {boolean}
   */
  function shouldStampPing(lastReceiveTs, pingSentTs) {
    if (!pingSentTs) return true; // no baseline yet → start one
    return lastReceiveTs >= pingSentTs; // previous ping answered → safe to restart
  }

  return { isStale, shouldStampPing };
})();

if (typeof module !== 'undefined') module.exports = DenWsLiveness;
