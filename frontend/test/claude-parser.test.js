const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

// Minimal DOM shim for testing
class Element {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.className = '';
    this.innerHTML = '';
    this._textContent = '';
    this.children = [];
    this.attributes = {};
    this._listeners = {};
  }
  // esc() relies on setting textContent then reading innerHTML
  get textContent() { return this._textContent; }
  set textContent(v) {
    this._textContent = v;
    this.innerHTML = String(v)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }
  appendChild(child) {
    this.children.push(child);
    return child;
  }
  setAttribute(k, v) {
    this.attributes[k] = v;
  }
  getAttribute(k) {
    return this.attributes[k];
  }
  addEventListener(event, fn) {
    this._listeners[event] = fn;
  }
  querySelector() {
    return null;
  }
  classList = {
    _classes: new Set(),
    add(...cs) { cs.forEach(c => this._classes.add(c)); },
    remove(...cs) { cs.forEach(c => this._classes.delete(c)); },
    toggle(c) { this._classes.has(c) ? this._classes.delete(c) : this._classes.add(c); },
  };
}

globalThis.document = {
  createElement(tag) {
    return new Element(tag);
  },
  createDocumentFragment() {
    return new Element('fragment');
  },
  querySelector() {
    return null;
  },
};

// DenIcons mock for Node.js test environment
globalThis.DenIcons = {
  zap: () => '[zap]',
  checkCircle: () => '[check]',
  xCircle: () => '[x]',
};

const ClaudeParser = require('../../frontend/js/claude-parser.js');

// --- parse ---

describe('ClaudeParser.parse', () => {
  it('parses valid JSON', () => {
    const result = ClaudeParser.parse('{"type":"system","subtype":"init"}');
    assert.deepStrictEqual(result, { type: 'system', subtype: 'init' });
  });

  it('returns null for invalid JSON', () => {
    assert.strictEqual(ClaudeParser.parse('not json'), null);
  });

  it('returns null for empty string', () => {
    assert.strictEqual(ClaudeParser.parse(''), null);
  });

  it('parses nested objects', () => {
    const input = JSON.stringify({ type: 'assistant', message: { content: [] } });
    const result = ClaudeParser.parse(input);
    assert.strictEqual(result.type, 'assistant');
    assert.deepStrictEqual(result.message.content, []);
  });
});

// --- renderEvent ---

describe('ClaudeParser.renderEvent', () => {
  it('returns null for null input', () => {
    assert.strictEqual(ClaudeParser.renderEvent(null), null);
  });

  it('returns null for unknown type', () => {
    assert.strictEqual(ClaudeParser.renderEvent({ type: 'unknown' }), null);
  });

  it('renders system init event', () => {
    const event = { type: 'system', subtype: 'init', model: 'claude-3', tools: ['a', 'b'] };
    const el = ClaudeParser.renderEvent(event);
    assert.ok(el);
    assert.ok(el.className.includes('msg-system'));
    assert.ok(el.innerHTML.includes('claude-3'));
    assert.ok(el.innerHTML.includes('2'));
  });

  it('skips non-init system events', () => {
    const event = { type: 'system', subtype: 'other' };
    assert.strictEqual(ClaudeParser.renderEvent(event), null);
  });

  it('renders assistant text', () => {
    const event = {
      type: 'assistant',
      message: { content: [{ type: 'text', text: 'Hello world' }] },
    };
    const el = ClaudeParser.renderEvent(event);
    assert.ok(el);
    assert.ok(el.className.includes('msg-assistant'));
    assert.ok(el.children.length > 0);
  });

  it('renders assistant tool_use', () => {
    const event = {
      type: 'assistant',
      message: {
        content: [{ type: 'tool_use', id: 'tool1', name: 'Bash', input: { command: 'ls' } }],
      },
    };
    const el = ClaudeParser.renderEvent(event);
    assert.ok(el);
    assert.ok(el.children.length > 0);
  });

  it('renders result event', () => {
    const event = {
      type: 'result',
      total_cost_usd: 0.0123,
      num_turns: 5,
      duration_ms: 12345,
      is_error: false,
    };
    const el = ClaudeParser.renderEvent(event);
    assert.ok(el);
    assert.ok(el.className.includes('msg-result'));
    assert.ok(el.innerHTML.includes('Done'));
    assert.ok(el.innerHTML.includes('$0.0123'));
    assert.ok(el.innerHTML.includes('5 turns'));
  });

  it('renders error result', () => {
    const event = { type: 'result', is_error: true, num_turns: 1 };
    const el = ClaudeParser.renderEvent(event);
    assert.ok(el);
    assert.ok(el.innerHTML.includes('Error'));
  });
});
