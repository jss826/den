const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const DenMarkdown = require('../js/markdown.js');

// --- renderMarkdown: 基本入力 ---

describe('renderMarkdown: empty/null input', () => {
  it('returns empty string for null', () => {
    assert.strictEqual(DenMarkdown.renderMarkdown(null), '');
  });

  it('returns empty string for undefined', () => {
    assert.strictEqual(DenMarkdown.renderMarkdown(undefined), '');
  });

  it('returns empty string for empty string', () => {
    assert.strictEqual(DenMarkdown.renderMarkdown(''), '');
  });

  it('returns empty string for 0', () => {
    assert.strictEqual(DenMarkdown.renderMarkdown(0), '');
  });
});

// --- renderMarkdown: HTML エスケープ ---

describe('renderMarkdown: HTML escaping', () => {
  it('escapes < and >', () => {
    const result = DenMarkdown.renderMarkdown('<b>bold</b>');
    assert.ok(!result.includes('<b>'));
    assert.ok(result.includes('&lt;b&gt;'));
  });

  it('escapes script tags', () => {
    const result = DenMarkdown.renderMarkdown('<script>alert("xss")</script>');
    assert.ok(!result.includes('<script>'));
    assert.ok(result.includes('&lt;script&gt;'));
  });

  it('escapes & character', () => {
    const result = DenMarkdown.renderMarkdown('a & b');
    assert.ok(result.includes('&amp;'));
  });

  it('escapes double quotes', () => {
    const result = DenMarkdown.renderMarkdown('a "quoted" b');
    assert.ok(result.includes('&quot;'));
  });

  it('handles all special chars together', () => {
    const result = DenMarkdown.renderMarkdown('<a href="x&y">');
    assert.ok(!result.includes('<a'));
    assert.ok(result.includes('&lt;'));
    assert.ok(result.includes('&gt;'));
    assert.ok(result.includes('&amp;'));
    assert.ok(result.includes('&quot;'));
  });
});

// --- renderMarkdown: コードブロック ---

describe('renderMarkdown: code blocks', () => {
  it('renders fenced code block', () => {
    const input = '```\nconst x = 1;\n```';
    const result = DenMarkdown.renderMarkdown(input);
    assert.ok(result.includes('<pre class="code-block">'));
    assert.ok(result.includes('<code>'));
    assert.ok(result.includes('const x = 1;'));
  });

  it('renders code block with language', () => {
    const input = '```js\nconst x = 1;\n```';
    const result = DenMarkdown.renderMarkdown(input);
    assert.ok(result.includes('class="language-js"'));
  });

  it('includes copy button', () => {
    const input = '```\ncode\n```';
    const result = DenMarkdown.renderMarkdown(input);
    assert.ok(result.includes('code-copy-btn'));
  });

  it('does not parse # inside code block as heading', () => {
    const input = '```\n# not a heading\n## also not\n```';
    const result = DenMarkdown.renderMarkdown(input);
    assert.ok(!result.includes('<h1>'));
    assert.ok(!result.includes('<h2>'));
    assert.ok(result.includes('# not a heading'));
  });

  it('does not parse - inside code block as list', () => {
    const input = '```\n- not a list item\n```';
    const result = DenMarkdown.renderMarkdown(input);
    assert.ok(!result.includes('<li>'));
    assert.ok(result.includes('- not a list item'));
  });

  it('preserves HTML entities inside code block', () => {
    const input = '```\n<div class="foo">&bar</div>\n```';
    const result = DenMarkdown.renderMarkdown(input);
    // User content <div> should be escaped inside code block
    assert.ok(result.includes('&lt;div class=&quot;foo&quot;&gt;'));
    assert.ok(result.includes('&amp;bar'));
  });
});

// --- renderMarkdown: インラインコード ---

describe('renderMarkdown: inline code', () => {
  it('renders inline code', () => {
    const result = DenMarkdown.renderMarkdown('use `npm install`');
    assert.ok(result.includes('<code>npm install</code>'));
  });

  it('does not parse markdown inside inline code', () => {
    const result = DenMarkdown.renderMarkdown('`**not bold**`');
    assert.ok(result.includes('<code>'));
    assert.ok(!result.includes('<strong>'));
  });

  it('handles multiple inline codes', () => {
    const result = DenMarkdown.renderMarkdown('`a` and `b`');
    assert.ok(result.includes('<code>a</code>'));
    assert.ok(result.includes('<code>b</code>'));
  });
});

// --- renderMarkdown: 見出し ---

describe('renderMarkdown: headings', () => {
  it('renders h1', () => {
    const result = DenMarkdown.renderMarkdown('# Title');
    assert.ok(result.includes('<h1>Title</h1>'));
  });

  it('renders h2', () => {
    const result = DenMarkdown.renderMarkdown('## Subtitle');
    assert.ok(result.includes('<h2>Subtitle</h2>'));
  });

  it('renders h3 through h6', () => {
    for (let i = 3; i <= 6; i++) {
      const hashes = '#'.repeat(i);
      const result = DenMarkdown.renderMarkdown(`${hashes} Heading ${i}`);
      assert.ok(result.includes(`<h${i}>Heading ${i}</h${i}>`));
    }
  });

  it('does not render 7+ hashes as heading', () => {
    const result = DenMarkdown.renderMarkdown('####### Not a heading');
    assert.ok(!result.includes('<h7>'));
  });

  it('requires space after #', () => {
    const result = DenMarkdown.renderMarkdown('#NoSpace');
    assert.ok(!result.includes('<h1>'));
  });

  it('trims trailing whitespace from heading text', () => {
    const result = DenMarkdown.renderMarkdown('## Title   ');
    assert.ok(result.includes('<h2>Title</h2>'));
  });
});

// --- renderMarkdown: リスト ---

describe('renderMarkdown: unordered list', () => {
  it('renders - items', () => {
    const result = DenMarkdown.renderMarkdown('- item1\n- item2');
    assert.ok(result.includes('<ul>'));
    assert.ok(result.includes('<li>item1</li>'));
    assert.ok(result.includes('<li>item2</li>'));
    assert.ok(result.includes('</ul>'));
  });

  it('renders * items', () => {
    const result = DenMarkdown.renderMarkdown('* item1\n* item2');
    assert.ok(result.includes('<ul>'));
    assert.ok(result.includes('<li>item1</li>'));
  });

  it('closes list when non-list line follows', () => {
    const result = DenMarkdown.renderMarkdown('- item\ntext');
    assert.ok(result.includes('</ul>'));
    assert.ok(result.includes('text'));
  });
});

describe('renderMarkdown: ordered list', () => {
  it('renders numbered items', () => {
    const result = DenMarkdown.renderMarkdown('1. first\n2. second');
    assert.ok(result.includes('<ol>'));
    assert.ok(result.includes('<li>first</li>'));
    assert.ok(result.includes('<li>second</li>'));
    assert.ok(result.includes('</ol>'));
  });

  it('closes ordered list on non-list line', () => {
    const result = DenMarkdown.renderMarkdown('1. item\ntext');
    assert.ok(result.includes('</ol>'));
  });

  it('uses start attribute when list starts from non-1 number', () => {
    const result = DenMarkdown.renderMarkdown('3. third\n4. fourth');
    assert.ok(result.includes('<ol start="3">'), 'should have start="3"');
    assert.ok(result.includes('<li>third</li>'));
    assert.ok(result.includes('<li>fourth</li>'));
  });

  it('omits start attribute when list starts from 1', () => {
    const result = DenMarkdown.renderMarkdown('1. first\n2. second');
    assert.ok(result.includes('<ol>'), 'should be plain <ol> without start');
    assert.ok(!result.includes('start='), 'should not have start attribute');
  });
});

// --- renderMarkdown: リンク ---

describe('renderMarkdown: links', () => {
  it('renders http link', () => {
    const result = DenMarkdown.renderMarkdown('[example](http://example.com)');
    assert.ok(result.includes('<a href="http://example.com"'));
    assert.ok(result.includes('target="_blank"'));
    assert.ok(result.includes('rel="noopener noreferrer"'));
    assert.ok(result.includes('>example</a>'));
  });

  it('renders https link', () => {
    const result = DenMarkdown.renderMarkdown('[site](https://example.com)');
    assert.ok(result.includes('href="https://example.com"'));
  });

  it('renders mailto link', () => {
    const result = DenMarkdown.renderMarkdown('[email](mailto:test@example.com)');
    assert.ok(result.includes('href="mailto:test@example.com"'));
  });

  it('blocks javascript: scheme', () => {
    const result = DenMarkdown.renderMarkdown('[click](javascript:alert(1))');
    // No <a> tag should be generated — URL shown as plain text fallback
    assert.ok(!result.includes('<a'));
  });

  it('blocks data: scheme', () => {
    const result = DenMarkdown.renderMarkdown('[click](data:text/html,<script>alert(1)</script>)');
    assert.ok(!result.includes('href="data:'));
  });

  it('blocks empty/relative URLs', () => {
    const result = DenMarkdown.renderMarkdown('[click](/etc/passwd)');
    assert.ok(!result.includes('<a'));
  });

  it('blocks ftp: scheme', () => {
    const result = DenMarkdown.renderMarkdown('[click](ftp://example.com)');
    assert.ok(!result.includes('<a'));
  });
});

// --- renderMarkdown: インライン書式 ---

describe('renderMarkdown: inline formatting', () => {
  it('renders bold', () => {
    const result = DenMarkdown.renderMarkdown('**bold text**');
    assert.ok(result.includes('<strong>bold text</strong>'));
  });

  it('renders italic', () => {
    const result = DenMarkdown.renderMarkdown('*italic text*');
    assert.ok(result.includes('<em>italic text</em>'));
  });

  it('renders bold and italic together', () => {
    const result = DenMarkdown.renderMarkdown('**bold** and *italic*');
    assert.ok(result.includes('<strong>bold</strong>'));
    assert.ok(result.includes('<em>italic</em>'));
  });
});

// --- renderMarkdown: 改行 ---

describe('renderMarkdown: line breaks', () => {
  it('converts newlines to <br>', () => {
    const result = DenMarkdown.renderMarkdown('line1\nline2');
    assert.ok(result.includes('line1<br>line2'));
  });

  it('does not add <br> inside list structures', () => {
    const result = DenMarkdown.renderMarkdown('- a\n- b');
    // Between </li> and next <li> there should be no <br>
    assert.ok(!result.includes('</li><br>'));
  });
});

// --- renderMarkdown: 複合テスト ---

describe('renderMarkdown: combined features', () => {
  it('renders heading with inline code', () => {
    const result = DenMarkdown.renderMarkdown('## Using `npm`');
    assert.ok(result.includes('<h2>'));
    assert.ok(result.includes('<code>npm</code>'));
  });

  it('renders list with bold text', () => {
    const result = DenMarkdown.renderMarkdown('- **important** item');
    assert.ok(result.includes('<li>'));
    assert.ok(result.includes('<strong>important</strong>'));
  });

  it('renders link inside text', () => {
    const result = DenMarkdown.renderMarkdown('See [docs](https://docs.example.com) for info');
    assert.ok(result.includes('<a href="https://docs.example.com"'));
    assert.ok(result.includes('See '));
    assert.ok(result.includes(' for info'));
  });
});
