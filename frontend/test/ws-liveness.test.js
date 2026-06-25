const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const DenWsLiveness = require('../js/ws-liveness.js');

// isStale(lastReceiveTs, pingSentTs, graceMs, now)
// Decides whether a WS connection is half-open: a ping was sent and the grace
// window elapsed with no inbound frame arriving since that ping.
describe('DenWsLiveness.isStale', () => {
  const GRACE = 8000;

  it('is not stale when no ping is outstanding (pingSentTs falsy)', () => {
    assert.equal(DenWsLiveness.isStale(0, 0, GRACE, 100000), false);
    assert.equal(DenWsLiveness.isStale(5000, 0, GRACE, 100000), false);
  });

  it('is not stale when a frame arrived at or after the ping was sent', () => {
    // ping sent at 1000, frame received at 1200 → alive
    assert.equal(DenWsLiveness.isStale(1200, 1000, GRACE, 1000 + GRACE + 1), false);
    // received exactly at ping time still counts as a response
    assert.equal(DenWsLiveness.isStale(1000, 1000, GRACE, 1000 + GRACE + 1), false);
  });

  it('is not stale during silence before the grace window elapses', () => {
    // ping at 1000, no frame since, only 7999ms passed (< 8000 grace)
    assert.equal(DenWsLiveness.isStale(500, 1000, GRACE, 1000 + GRACE - 1), false);
  });

  it('is stale when the grace window elapses with silence (boundary)', () => {
    // ping at 1000, no frame since, exactly grace elapsed
    assert.equal(DenWsLiveness.isStale(500, 1000, GRACE, 1000 + GRACE), true);
  });

  it('is stale when silence exceeds the grace window', () => {
    assert.equal(DenWsLiveness.isStale(500, 1000, GRACE, 1000 + GRACE + 5000), true);
  });

  it('is stale when the last receive predates the current ping and grace elapsed', () => {
    // a stale earlier receive (200) must not be mistaken for a response to the
    // ping sent at 1000
    assert.equal(DenWsLiveness.isStale(200, 1000, GRACE, 1000 + GRACE), true);
  });
});

// shouldStampPing(lastReceiveTs, pingSentTs)
// Decides whether a freshly sent ping should (re)start the liveness window.
// It should restart only when there is no still-unanswered prior ping, so a
// burst of pings (e.g. repeated resume events) can't keep pushing the baseline
// forward and indefinitely defer half-open detection.
describe('DenWsLiveness.shouldStampPing', () => {
  it('stamps when no ping has been sent yet (pingSentTs falsy)', () => {
    assert.equal(DenWsLiveness.shouldStampPing(5000, 0), true);
  });

  it('stamps when the previous ping was answered (received at/after it)', () => {
    assert.equal(DenWsLiveness.shouldStampPing(2000, 1000), true);
    assert.equal(DenWsLiveness.shouldStampPing(1000, 1000), true);
  });

  it('does NOT stamp while a prior ping is still unanswered', () => {
    // ping sent at 1000, last inbound frame was at 200 (before it) → still waiting
    assert.equal(DenWsLiveness.shouldStampPing(200, 1000), false);
  });
});
