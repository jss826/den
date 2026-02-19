// Den - 簡易 Markdown レンダラー
// コードブロック、インラインコード、太字、イタリック、見出し、リスト、リンク
const DenMarkdown = (() => {

  const ESC_RE = /[&<>"]/g;
  const ESC_MAP = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' };
  function esc(str) {
    return String(str).replace(ESC_RE, c => ESC_MAP[c]);
  }

  function renderMarkdown(text) {
    if (!text) return '';
    // NUL 文字を除去（プレースホルダー \x00CB..\x00 / \x00IC..\x00 との衝突防止）
    let html = esc(text.replace(/\x00/g, ''));

    // コードブロック ```lang\n...\n``` — プレースホルダーで退避
    const codeBlocks = [];
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
      const idx = codeBlocks.length;
      const langAttr = lang ? ` class="language-${lang}"` : '';
      codeBlocks.push('<div class="code-block-wrapper"><pre class="code-block"><code' +
        langAttr + '>' + code + '</code></pre><button class="code-copy-btn">Copy</button></div>');
      return '\x00CB' + idx + '\x00';
    });

    // インラインコード — プレースホルダーで退避
    const inlineCodes = [];
    html = html.replace(/`([^`]+)`/g, (_, code) => {
      const idx = inlineCodes.length;
      inlineCodes.push('<code>' + code + '</code>');
      return '\x00IC' + idx + '\x00';
    });

    // 行単位処理（見出し、リスト）
    const lines = html.split('\n');
    const result = [];
    let inUl = false, inOl = false;

    for (let i = 0; i < lines.length; i++) {
      let line = lines[i];
      const ch = line.charAt(0);

      // 見出し: #{1-6} text（先頭が # の行のみ regex 評価）
      if (ch === '#') {
        const headingMatch = line.match(/^(#{1,6}) (.+?)\s*$/);
        if (headingMatch) {
          if (inUl) { result.push('</ul>'); inUl = false; }
          if (inOl) { result.push('</ol>'); inOl = false; }
          const level = headingMatch[1].length;
          result.push(`<h${level}>${headingMatch[2]}</h${level}>`);
          continue;
        }
      }

      // 順序なしリスト: - text / * text（先頭が - or * の行のみ）
      if (ch === '-' || ch === '*') {
        const ulMatch = line.match(/^[-*] (.+)$/);
        if (ulMatch) {
          if (inOl) { result.push('</ol>'); inOl = false; }
          if (!inUl) { result.push('<ul>'); inUl = true; }
          result.push(`<li>${ulMatch[1]}</li>`);
          continue;
        }
      }

      // 順序付きリスト: 1. text（先頭が数字の行のみ）
      if (ch >= '0' && ch <= '9') {
        const olMatch = line.match(/^(\d+)\. (.+)$/);
        if (olMatch) {
          if (inUl) { result.push('</ul>'); inUl = false; }
          if (!inOl) {
            const start = parseInt(olMatch[1], 10);
            result.push(start === 1 ? '<ol>' : `<ol start="${start}">`);
            inOl = true;
          }
          result.push(`<li>${olMatch[2]}</li>`);
          continue;
        }
      }

      // リスト外の行 — 開いてるリストを閉じる
      if (inUl) { result.push('</ul>'); inUl = false; }
      if (inOl) { result.push('</ol>'); inOl = false; }
      result.push(line);
    }
    if (inUl) result.push('</ul>');
    if (inOl) result.push('</ol>');

    html = result.join('\n');

    // リンク: [text](url) — 安全なスキームのみ許可
    // 注意: text, url は冒頭の esc(text) (line 16) により既に HTML エスケープ済み。
    // 再度 esc() を適用すると二重エスケープになるため適用しない。
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, text, url) => {
      const trimmed = url.trim().toLowerCase();
      if (trimmed.startsWith('http://') || trimmed.startsWith('https://') || trimmed.startsWith('mailto:')) {
        return `<a href="${url.trim()}" target="_blank" rel="noopener noreferrer">${text}</a>`;
      }
      return `${text} (${url})`;
    });

    // 太字: **text**
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');

    // イタリック: *text* （太字の後に処理して衝突回避）
    html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');

    // 改行（リスト要素内は不要なので<li>/<h>タグの直後の改行は除去）
    html = html.replace(/(<\/(?:li|ul|ol|h[1-6])>)\n/g, '$1');
    html = html.replace(/\n(<(?:ul|ol)>)/g, '$1');
    html = html.replace(/\n/g, '<br>');

    // プレースホルダー復元
    html = html.replace(/\x00IC(\d+)\x00/g, (_, idx) => inlineCodes[parseInt(idx, 10)]);
    html = html.replace(/\x00CB(\d+)\x00/g, (_, idx) => codeBlocks[parseInt(idx, 10)]);

    return html;
  }

  // DOMParser ベースの HTML サニタイザー（defense-in-depth）
  // renderMarkdown が生成するタグ・属性のみホワイトリストで許可
  const ALLOWED_TAGS = new Set([
    'h1','h2','h3','h4','h5','h6','br','strong','em',
    'code','pre','ul','ol','li','a','div','button',
  ]);
  const ALLOWED_ATTRS = {
    a: new Set(['href','target','rel']),
    code: new Set(['class']),
    pre: new Set(['class']),
    div: new Set(['class']),
    button: new Set(['class']),
    ol: new Set(['start']),
  };
  const SAFE_SCHEMES = /^(https?:|mailto:)/i;

  function sanitize(html) {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    (function walk(parent) {
      for (const node of [...parent.childNodes]) {
        if (node.nodeType !== 1) continue;           // テキスト/コメントはそのまま
        const tag = node.tagName.toLowerCase();
        if (!ALLOWED_TAGS.has(tag)) {
          node.replaceWith(...node.childNodes);       // 不許可タグ → 子ノードで置換
          continue;
        }
        const allowed = ALLOWED_ATTRS[tag];
        for (const a of [...node.attributes]) {
          if (!allowed || !allowed.has(a.name)) node.removeAttribute(a.name);
        }
        if (tag === 'a') {
          const href = (node.getAttribute('href') || '').trim();
          if (!SAFE_SCHEMES.test(href)) node.removeAttribute('href');
        }
        walk(node);
      }
    })(doc.body);
    return doc.body.innerHTML;
  }

  return { renderMarkdown, sanitize };
})();
if (typeof module !== 'undefined') module.exports = DenMarkdown;
