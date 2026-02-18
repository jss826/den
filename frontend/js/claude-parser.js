/* global DenIcons */
// Den - Claude streaming-json パーサー + レンダラー
const ClaudeParser = (() => {

  // streaming-json の1行をパースして表示用オブジェクトに変換
  function parse(line) {
    try {
      return JSON.parse(line);
    } catch {
      return null;
    }
  }

  // メッセージをHTML要素にレンダリング
  function renderEvent(event) {
    if (!event) return null;

    switch (event.type) {
      case 'system':
        return renderSystem(event);
      case 'assistant':
        return renderAssistant(event);
      case 'user':
        return renderToolResult(event);
      case 'result':
        return renderResult(event);
      case 'user_prompt':
        return renderUserPrompt(event);
      default:
        return null;
    }
  }

  function renderSystem(event) {
    if (event.subtype !== 'init') return null;
    const div = el('div', 'msg msg-system');
    const model = event.model || 'unknown';
    const tools = (event.tools || []).length;
    div.innerHTML = `<span class="msg-label">System</span> Model: ${esc(model)} | Tools: ${tools}`;
    return div;
  }

  function renderAssistant(event) {
    const container = el('div', 'msg msg-assistant');
    const contents = event.message?.content || [];

    for (const block of contents) {
      if (block.type === 'text') {
        const textDiv = el('div', 'msg-text');
        textDiv.innerHTML = renderMarkdown(block.text);
        container.appendChild(textDiv);
      } else if (block.type === 'tool_use') {
        container.appendChild(renderToolUse(block));
      }
    }
    return container;
  }

  function renderToolUse(block) {
    const div = el('div', 'tool-use');
    const header = el('div', 'tool-header');
    header.innerHTML = `<span class="tool-icon">${DenIcons.zap(14)}</span><span class="tool-name">${esc(block.name)}</span>`;

    const body = el('div', 'tool-body collapsed');
    body.setAttribute('data-tool-id', block.id);

    // ツール入力の表示
    const inputStr = formatToolInput(block.name, block.input);
    const pre = el('pre', 'tool-input');
    pre.textContent = inputStr;
    body.appendChild(pre);

    // 折りたたみトグル
    header.addEventListener('click', () => {
      body.classList.toggle('collapsed');
    });

    div.appendChild(header);
    div.appendChild(body);
    return div;
  }

  function formatToolInput(name, input) {
    if (!input) return '';
    switch (name) {
      case 'Bash':
        return input.command || JSON.stringify(input, null, 2);
      case 'Read':
        return input.file_path || JSON.stringify(input, null, 2);
      case 'Write':
      case 'Edit':
        return input.file_path || JSON.stringify(input, null, 2);
      case 'Glob':
        return input.pattern || JSON.stringify(input, null, 2);
      case 'Grep':
        return input.pattern || JSON.stringify(input, null, 2);
      default:
        return JSON.stringify(input, null, 2);
    }
  }

  function renderToolResult(event) {
    const contents = event.message?.content || [];
    const container = document.createDocumentFragment();

    for (const block of contents) {
      if (block.type !== 'tool_result') continue;

      // 対応する tool_use の body を探す
      const toolBody = block.tool_use_id
        ? document.querySelector(`[data-tool-id="${CSS.escape(block.tool_use_id)}"]`)
        : null;
      if (toolBody) {
        const resultDiv = el('div', 'tool-result' + (block.is_error ? ' error' : ''));
        const pre = el('pre', 'tool-output');
        const content = typeof block.content === 'string'
          ? block.content
          : JSON.stringify(block.content, null, 2);
        pre.textContent = truncate(content, 2000);
        resultDiv.appendChild(pre);
        toolBody.appendChild(resultDiv);
        // 結果が来たらステータス表示更新
        const header = toolBody.previousElementSibling;
        if (header) {
          const icon = header.querySelector('.tool-icon');
          if (icon) {
            icon.innerHTML = block.is_error ? DenIcons.xCircle(14) : DenIcons.checkCircle(14);
            icon.classList.remove('success', 'error');
            icon.classList.add(block.is_error ? 'error' : 'success');
          }
        }
      }
    }
    return container.children.length > 0 ? container : null;
  }

  function renderResult(event) {
    const div = el('div', 'msg msg-result');
    const cost = event.total_cost_usd != null ? `$${event.total_cost_usd.toFixed(4)}` : '-';
    const turns = event.num_turns || 0;
    const duration = event.duration_ms ? `${(event.duration_ms / 1000).toFixed(1)}s` : '-';
    const status = event.is_error ? 'Error' : 'Done';
    div.innerHTML = `<span class="result-status ${event.is_error ? 'error' : ''}">${status}</span>
      <span class="result-meta">${turns} turns | ${duration} | ${cost}</span>`;
    return div;
  }

  function renderUserPrompt(event) {
    const div = el('div', 'msg msg-user');
    div.textContent = event.prompt || '';
    return div;
  }

  // 簡易 Markdown レンダラー（コードブロック、インラインコード、太字、イタリック、見出し、リスト、リンク）
  function renderMarkdown(text) {
    if (!text) return '';
    let html = esc(text);

    // コードブロック ```lang\n...\n``` — プレースホルダーで退避
    const codeBlocks = [];
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, _lang, code) => {
      const idx = codeBlocks.length;
      codeBlocks.push('<div class="code-block-wrapper"><pre class="code-block"><code>' +
        code + '</code></pre><button class="code-copy-btn">Copy</button></div>');
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

    // リンク: [text](url) — 安全なスキームのみ許可
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, text, url) => {
      const trimmed = url.trim().toLowerCase();
      if (trimmed.startsWith('http://') || trimmed.startsWith('https://') || trimmed.startsWith('mailto:')) {
        return `<a href="${url}" target="_blank" rel="noopener">${text}</a>`;
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

  // ユーティリティ
  function el(tag, className) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    return e;
  }

  function esc(str) {
    const d = document.createElement('div');
    d.textContent = str;
    return d.innerHTML;
  }

  function truncate(str, max) {
    if (str.length <= max) return str;
    return str.slice(0, max) + '\n... (truncated)';
  }

  return { parse, renderEvent, renderMarkdown };
})();
if (typeof module !== 'undefined') module.exports = ClaudeParser;
