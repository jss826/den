const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const ClipboardHistory = require('../js/clipboard-history.js');
const formatTime = ClipboardHistory.formatTime;

describe('formatTime', () => {
  const now = 1700000000000; // fixed reference point

  it('returns "just now" for < 60s ago', () => {
    assert.strictEqual(formatTime(now - 30000, now), 'just now');
  });

  it('returns minutes ago for < 1h', () => {
    assert.strictEqual(formatTime(now - 300000, now), '5m ago');
  });

  it('returns hours ago for < 24h', () => {
    assert.strictEqual(formatTime(now - 7200000, now), '2h ago');
  });

  it('returns locale date string for >= 24h', () => {
    const result = formatTime(now - 86400001, now);
    // Should not contain 'm ago' or 'h ago' or 'just now'
    assert.ok(!result.includes('m ago'));
    assert.ok(!result.includes('h ago'));
    assert.ok(!result.includes('just now'));
  });

  // Boundary values
  it('59999ms → "just now"', () => {
    assert.strictEqual(formatTime(now - 59999, now), 'just now');
  });

  it('60000ms → "1m ago"', () => {
    assert.strictEqual(formatTime(now - 60000, now), '1m ago');
  });

  it('3599999ms → "59m ago"', () => {
    assert.strictEqual(formatTime(now - 3599999, now), '59m ago');
  });

  it('3600000ms → "1h ago"', () => {
    assert.strictEqual(formatTime(now - 3600000, now), '1h ago');
  });

  it('86399999ms → "23h ago"', () => {
    assert.strictEqual(formatTime(now - 86399999, now), '23h ago');
  });

  it('86400000ms → locale date string', () => {
    const result = formatTime(now - 86400000, now);
    assert.ok(!result.includes('h ago'));
  });
});
