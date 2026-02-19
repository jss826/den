// Den - 簡易 Markdown レンダラー
// コードブロック、インラインコード、太字、イタリック、見出し、リスト、リンク
// eslint-disable-next-line no-unused-vars
const DenMarkdown = (() => {

  function esc(str) {
    return String(str)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function renderMarkdown(text) {
    if (!text) return '';
    let html = esc(text);

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

      // 見出し: #{1-6} text
      const headingMatch = line.match(/^(#{1,6}) (.+)$/);
      if (headingMatch) {
        if (inUl) { result.push('</ul>'); inUl = false; }
        if (inOl) { result.push('</ol>'); inOl = false; }
        const level = headingMatch[1].length;
        result.push(`<h${level}>${headingMatch[2]}</h${level}>`);
        continue;
      }

      // 順序なしリスト: - text / * text
      const ulMatch = line.match(/^[-*] (.+)$/);
      if (ulMatch) {
        if (inOl) { result.push('</ol>'); inOl = false; }
        if (!inUl) { result.push('<ul>'); inUl = true; }
        result.push(`<li>${ulMatch[1]}</li>`);
        continue;
      }

      // 順序付きリスト: 1. text
      const olMatch = line.match(/^\d+\. (.+)$/);
      if (olMatch) {
        if (inUl) { result.push('</ul>'); inUl = false; }
        if (!inOl) { result.push('<ol>'); inOl = true; }
        result.push(`<li>${olMatch[1]}</li>`);
        continue;
      }

      // リスト外の行 — 開いてるリストを閉じる
      if (inUl) { result.push('</ul>'); inUl = false; }
      if (inOl) { result.push('</ol>'); inOl = false; }
      result.push(line);
    }
    if (inUl) result.push('</ul>');
    if (inOl) result.push('</ol>');

    html = result.join('\n');

    // リンク: [text](url) — 安全なスキームのみ許可（URL は esc() でエスケープ）
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, text, url) => {
      const trimmed = url.trim().toLowerCase();
      if (trimmed.startsWith('http://') || trimmed.startsWith('https://') || trimmed.startsWith('mailto:')) {
        return `<a href="${esc(url.trim())}" target="_blank" rel="noopener">${text}</a>`;
      }
      return `${text} (${esc(url)})`;
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

  return { renderMarkdown };
})();
