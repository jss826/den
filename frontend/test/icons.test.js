const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const DenIcons = require('../js/icons.js');

describe('DenIcons.svg', () => {
  it('returns a valid SVG string with default size', () => {
    const result = DenIcons.svg('<circle cx="12" cy="12" r="3"/>');
    assert.ok(result.startsWith('<svg'));
    assert.ok(result.includes('width="16"'));
    assert.ok(result.includes('height="16"'));
    assert.ok(result.includes('viewBox="0 0 24 24"'));
    assert.ok(result.includes('</svg>'));
  });

  it('uses custom size when provided', () => {
    const result = DenIcons.svg('<line x1="0" y1="0" x2="24" y2="24"/>', 24);
    assert.ok(result.includes('width="24"'));
    assert.ok(result.includes('height="24"'));
  });

  it('includes inner content', () => {
    const inner = '<path d="M0 0"/>';
    const result = DenIcons.svg(inner);
    assert.ok(result.includes(inner));
  });
});

describe('DenIcons.fileColor', () => {
  it('returns JS color for .js files', () => {
    assert.strictEqual(DenIcons.fileColor('app.js'), '#f7df1e');
  });

  it('returns Rust color for .rs files', () => {
    assert.strictEqual(DenIcons.fileColor('main.rs'), '#dea584');
  });

  it('returns null for unknown extension', () => {
    assert.strictEqual(DenIcons.fileColor('unknown.xyz'), null);
  });

  it('handles files with multiple dots', () => {
    assert.strictEqual(DenIcons.fileColor('file.test.js'), '#f7df1e');
  });

  it('returns Python color for .py files', () => {
    assert.strictEqual(DenIcons.fileColor('script.py'), '#3572a5');
  });

  it('returns TypeScript color for .ts files', () => {
    assert.strictEqual(DenIcons.fileColor('index.ts'), '#3178c6');
  });
});

describe('DenIcons icon functions', () => {
  const iconFns = [
    'filePlus', 'folderPlus', 'upload', 'refresh', 'gear', 'terminal',
    'chevronLeft', 'chevronRight', 'download',
    'panelLeft', 'panelRight', 'snippet', 'clipboard',
    'folder', 'file',
  ];

  for (const name of iconFns) {
    it(`${name}() returns an SVG string`, () => {
      const result = DenIcons[name]();
      assert.ok(typeof result === 'string');
      assert.ok(result.includes('<svg'));
      assert.ok(result.includes('</svg>'));
    });
  }
});
