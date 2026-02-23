const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const DenKeyPresets = require('../js/settings-presets.js');

describe('DenKeyPresets.getPresets', () => {
  it('returns a non-empty array', () => {
    const presets = DenKeyPresets.getPresets();
    assert.ok(Array.isArray(presets));
    assert.ok(presets.length > 0);
  });

  it('every preset has a display property', () => {
    const presets = DenKeyPresets.getPresets();
    for (const p of presets) {
      assert.ok(typeof p.display === 'string' && p.display.length > 0,
        `preset missing display: ${JSON.stringify(p)}`);
    }
  });

  it('non-stack presets have label and send', () => {
    const presets = DenKeyPresets.getPresets();
    const nonStack = presets.filter(p => p.type !== 'stack');
    assert.ok(nonStack.length > 0);
    for (const p of nonStack) {
      assert.ok(typeof p.label === 'string' && p.label.length > 0,
        `non-stack preset missing label: ${JSON.stringify(p)}`);
      assert.ok(typeof p.send === 'string',
        `non-stack preset missing send: ${JSON.stringify(p)}`);
    }
  });

  it('stack presets have items array with 2+ items', () => {
    const presets = DenKeyPresets.getPresets();
    const stacks = presets.filter(p => p.type === 'stack');
    assert.ok(stacks.length > 0);
    for (const p of stacks) {
      assert.ok(Array.isArray(p.items), `stack preset missing items: ${JSON.stringify(p)}`);
      assert.ok(p.items.length >= 2,
        `stack preset has < 2 items: ${p.display}`);
    }
  });

  it('returns a new reference each time (mutation safety)', () => {
    const a = DenKeyPresets.getPresets();
    const b = DenKeyPresets.getPresets();
    assert.notStrictEqual(a, b);
    // Mutating one should not affect the other
    a[0].display = '__mutated__';
    assert.notStrictEqual(b[0].display, '__mutated__');
  });

  it('returns deep copies of stack items', () => {
    const a = DenKeyPresets.getPresets();
    const stack = a.find(p => p.type === 'stack');
    const b = DenKeyPresets.getPresets();
    const stackB = b.find(p => p.type === 'stack' && p.display === stack.display);
    assert.notStrictEqual(stack.items, stackB.items);
    stack.items[0].label = '__mutated__';
    assert.notStrictEqual(stackB.items[0].label, '__mutated__');
  });
});
