// Den - キーバープリセット管理モジュール
const DenKeyPresets = (() => {
  /** @type {Array<{label?: string, send?: string, display: string, type?: string, action?: string, items?: Array}>} */
  const KEY_PRESETS = [
    { label: 'Tab', send: '\\t', display: 'Tab (\\t)' },
    { label: 'Esc', send: '\\x1b', display: 'Esc (\\x1b)' },
    { label: '\u2191', send: '\\x1b[A', display: '\u2191 (Up arrow)' },
    { label: '\u2193', send: '\\x1b[B', display: '\u2193 (Down arrow)' },
    { label: '\u2192', send: '\\x1b[C', display: '\u2192 (Right arrow)' },
    { label: '\u2190', send: '\\x1b[D', display: '\u2190 (Left arrow)' },
    { label: '|', send: '|', display: '| (Pipe)' },
    { label: '~', send: '~', display: '~ (Tilde)' },
    { label: '/', send: '/', display: '/ (Slash)' },
    { label: '-', send: '-', display: '- (Hyphen)' },
    { label: '_', send: '_', display: '_ (Underscore)' },
    { label: '.', send: '.', display: '. (Dot)' },
    { label: ':', send: ':', display: ': (Colon)' },
    { label: '=', send: '=', display: '= (Equals)' },
    { label: 'C-c', send: '\\x03', display: 'Ctrl+C (\\x03)' },
    { label: 'C-d', send: '\\x04', display: 'Ctrl+D (\\x04)' },
    { label: 'C-z', send: '\\x1a', display: 'Ctrl+Z (\\x1a)' },
    { label: 'C-l', send: '\\x0c', display: 'Ctrl+L (\\x0c)' },
    { label: 'C-a', send: '\\x01', display: 'Ctrl+A (\\x01)' },
    { label: 'C-e', send: '\\x05', display: 'Ctrl+E (\\x05)' },
    { label: 'C-r', send: '\\x12', display: 'Ctrl+R (\\x12)' },
    { label: 'C-w', send: '\\x17', display: 'Ctrl+W (\\x17)' },
    { label: 'C-u', send: '\\x15', display: 'Ctrl+U (\\x15)' },
    { label: 'C-k', send: '\\x0b', display: 'Ctrl+K (\\x0b)' },
    { label: 'Enter', send: '\\r', display: 'Enter (\\r)' },
    { label: 'BS', send: '\\x7f', display: 'Backspace (\\x7f)' },
    { label: 'Del', send: '\\x1b[3~', display: 'Delete' },
    { label: 'Ins', send: '\\x1b[2~', display: 'Insert' },
    { label: 'Home', send: '\\x1b[H', display: 'Home' },
    { label: 'End', send: '\\x1b[F', display: 'End' },
    { label: 'PgUp', send: '\\x1b[5~', display: 'Page Up' },
    { label: 'PgDn', send: '\\x1b[6~', display: 'Page Down' },
    { label: 'F1', send: '\\x1bOP', display: 'F1' },
    { label: 'F2', send: '\\x1bOQ', display: 'F2' },
    { label: 'F3', send: '\\x1bOR', display: 'F3' },
    { label: 'F4', send: '\\x1bOS', display: 'F4' },
    { label: 'F5', send: '\\x1b[15~', display: 'F5' },
    { label: 'F6', send: '\\x1b[17~', display: 'F6' },
    { label: 'F7', send: '\\x1b[18~', display: 'F7' },
    { label: 'F8', send: '\\x1b[19~', display: 'F8' },
    { label: 'F9', send: '\\x1b[20~', display: 'F9' },
    { label: 'F10', send: '\\x1b[21~', display: 'F10' },
    { label: 'F11', send: '\\x1b[23~', display: 'F11' },
    { label: 'F12', send: '\\x1b[24~', display: 'F12' },
    { label: 'S-Tab', send: '\\x1b[Z', display: 'Shift+Tab' },
    { label: 'Copy', send: '', display: 'Copy (selection)', type: 'action', action: 'copy' },
    { label: 'Paste', send: '', display: 'Paste (clipboard)', type: 'action', action: 'paste' },
    { label: 'Sel', send: '', display: 'Select mode (tap lines)', type: 'action', action: 'select' },
    { label: 'Screen', send: '', display: 'Copy screen (visible)', type: 'action', action: 'copy-screen' },
    { label: 'Sc\u2191', send: '', display: 'Scroll page up', type: 'action', action: 'scroll-page-up' },
    { label: 'Sc\u2193', send: '', display: 'Scroll page down', type: 'action', action: 'scroll-page-down' },
    { label: 'Top', send: '', display: 'Scroll to top', type: 'action', action: 'scroll-top' },
    { label: 'Bot', send: '', display: 'Scroll to bottom', type: 'action', action: 'scroll-bottom' },
    // スタックプリセット
    { display: 'C-c/C-z (Interrupt/Suspend)', type: 'stack', items: [
        { label: 'C-c', send: '\\x03', display: 'Ctrl+C' },
        { label: 'C-z', send: '\\x1a', display: 'Ctrl+Z' },
      ] },
    { display: 'C-d/C-l (EOF/Clear)', type: 'stack', items: [
        { label: 'C-d', send: '\\x04', display: 'Ctrl+D' },
        { label: 'C-l', send: '\\x0c', display: 'Ctrl+L' },
      ] },
    { display: 'Top/Bot (Scroll Top/Bottom)', type: 'stack', items: [
        { label: 'Top', send: '', type: 'action', action: 'scroll-top', display: 'Scroll to top' },
        { label: 'Bot', send: '', type: 'action', action: 'scroll-bottom', display: 'Scroll to bottom' },
      ] },
    { display: 'Enter/A-Ent/C-j (Enter / Alt+Enter / Newline)', type: 'stack', items: [
        { label: 'Enter', send: '\\r' },
        { label: 'A-Ent', send: '\\x1b\\r', display: 'Alt+Enter' },
        { label: 'C-j', send: '\\x0a', display: 'Ctrl+J (Newline)' },
      ] },
  ];

  /** @returns {Array} プリセット配列のディープコピー */
  function getPresets() {
    return KEY_PRESETS.map(p => {
      const copy = { ...p };
      if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
      return copy;
    });
  }

  /**
   * プリセット選択 <select> にオプションを生成する（初回のみ）。
   * @param {HTMLSelectElement} presetSelect - 単一キー用プリセットドロップダウン
   * @param {HTMLSelectElement|null} stackPreset - スタックアイテム用プリセットドロップダウン
   */
  function setupAddForm(presetSelect, stackPreset) {
    if (!presetSelect) return;
    // F006: presetSelect と stackPreset を独立して冪等判定
    const presetDone = presetSelect.options.length > 1;
    const stackDone = !stackPreset || stackPreset.options.length > 1;
    if (presetDone && stackDone) return;

    const nonStackPresets = KEY_PRESETS.filter(p => p.type !== 'stack');
    const stackPresets = KEY_PRESETS.filter(p => p.type === 'stack');

    if (!presetDone) {
      nonStackPresets.forEach(p => {
        const opt = document.createElement('option');
        if (p.type === 'action') {
          opt.value = '__action:' + p.action;
          opt.dataset.btnType = p.type;
          opt.dataset.btnAction = p.action;
        } else {
          opt.value = p.send;
        }
        opt.textContent = p.display;
        if (p.label) opt.dataset.label = p.label;
        presetSelect.appendChild(opt);
      });

      stackPresets.forEach(p => {
        const opt = document.createElement('option');
        opt.value = '__stack:' + p.display;
        opt.dataset.btnType = 'stack';
        opt.dataset.stackItems = JSON.stringify(p.items);
        opt.textContent = p.display;
        presetSelect.appendChild(opt);
      });
    }

    // Populate stack-item preset dropdown (non-stack only, independent check)
    if (stackPreset && !stackDone) {
      nonStackPresets.forEach(p => {
        const opt = document.createElement('option');
        opt.value = p.send;
        opt.textContent = p.display;
        if (p.label) opt.dataset.label = p.label;
        if (p.type === 'action') {
          opt.dataset.btnType = 'action';
          opt.dataset.btnAction = p.action;
        }
        stackPreset.appendChild(opt);
      });
    }
  }

  /**
   * スタックビルダーのアイテム一覧を描画する。
   * @param {HTMLElement} container - アイテム表示コンテナ
   * @param {Array} items - 編集中のスタックアイテム配列
   * @param {function(number): void} onRemove - アイテム削除コールバック（index）
   */
  function renderStackItems(container, items, onRemove) {
    if (!container) return;
    container.innerHTML = '';
    items.forEach((item, idx) => {
      const div = document.createElement('div');
      div.className = 'keybar-stack-item';
      const label = document.createElement('span');
      label.textContent = item.display || item.label;
      div.appendChild(label);
      const removeBtn = document.createElement('button');
      removeBtn.className = 'keybar-stack-item-remove';
      removeBtn.type = 'button';
      removeBtn.textContent = '\u00d7';
      removeBtn.addEventListener('click', () => {
        onRemove(idx);
      });
      div.appendChild(removeBtn);
      container.appendChild(div);
    });
  }

  return { getPresets, setupAddForm, renderStackItems };
})();

if (typeof module !== 'undefined') module.exports = DenKeyPresets;
