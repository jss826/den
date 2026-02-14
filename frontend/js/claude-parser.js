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

  // 簡易 Markdown レンダラー（コードブロック、インラインコード、太字、リンク）
  function renderMarkdown(text) {
    if (!text) return '';
    let html = esc(text);

    // コードブロック ```lang\n...\n```
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
      return '<div class="code-block-wrapper"><pre class="code-block"><code>' +
        code + '</code></pre><button class="code-copy-btn">Copy</button></div>';
    });

    // インラインコード
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');

    // 太字
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');

    // 改行
    html = html.replace(/\n/g, '<br>');

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

  return { parse, renderEvent };
})();
if (typeof module !== 'undefined') module.exports = ClaudeParser;
