// Den - タッチキーバーモジュール
const Keybar = (() => {
  let container = null;
  let modifiers = { ctrl: false, alt: false, shift: false };
  let activeKeys = [];

  // スクロールアクション → ターミナルメソッドのディスパッチマップ
  const SCROLL_ACTIONS = {
    'scroll-page-up':   t => t.scrollPages(-1),
    'scroll-page-down': t => t.scrollPages(1),
    'scroll-top':       t => t.scrollToTop(),
    'scroll-bottom':    t => t.scrollToBottom(),
  };

  // デフォルトキー配列
  const DEFAULT_KEYS = [
    { label: 'Ctrl', send: '', type: 'modifier', mod_key: 'ctrl' },
    { label: 'Alt', send: '', type: 'modifier', mod_key: 'alt' },
    { label: 'Shift', send: '', type: 'modifier', mod_key: 'shift' },
    { label: 'Tab', send: '\t' },
    { label: 'Esc', send: '\x1b' },
    { label: '\u2191', send: '\x1b[A', display: 'Up arrow' },
    { label: '\u2193', send: '\x1b[B', display: 'Down arrow' },
    { label: '\u2192', send: '\x1b[C', display: 'Right arrow' },
    { label: '\u2190', send: '\x1b[D', display: 'Left arrow' },
    { label: '|', send: '|', display: 'Pipe' },
    { label: '~', send: '~', display: 'Tilde' },
    { label: '/', send: '/' },
    { label: '-', send: '-' },
    { label: 'C-c', send: '\x03', display: 'Ctrl+C' },
    { label: 'C-d', send: '\x04', display: 'Ctrl+D' },
    { label: 'C-z', send: '\x1a', display: 'Ctrl+Z' },
    { label: 'C-l', send: '\x0c', display: 'Ctrl+L' },
    { label: 'Sc\u2191', send: '', type: 'action', action: 'scroll-page-up', display: 'Scroll page up' },
    { label: 'Sc\u2193', send: '', type: 'action', action: 'scroll-page-down', display: 'Scroll page down' },
    { label: 'Top', send: '', type: 'action', action: 'scroll-top', display: 'Scroll to top' },
    { label: 'Bot', send: '', type: 'action', action: 'scroll-bottom', display: 'Scroll to bottom' },
    { label: 'Paste', send: '', type: 'action', action: 'paste', display: 'Paste (clipboard)' },
    { label: 'Sel', send: '', type: 'action', action: 'select', display: 'Select mode' },
    { label: 'Screen', send: '', type: 'action', action: 'copy-screen', display: 'Copy screen' },
  ];

  function init(el, customKeys) {
    container = el;
    activeKeys = customKeys && customKeys.length > 0 ? customKeys : DEFAULT_KEYS;
    render();

    if (isTouchDevice()) {
      container.classList.add('visible');
    }
  }

  function reload(customKeys) {
    activeKeys = customKeys && customKeys.length > 0 ? customKeys : DEFAULT_KEYS;
    render();
  }

  function getDefaultKeys() {
    return DEFAULT_KEYS.map(k => ({ ...k }));
  }

  function isTouchDevice() {
    return 'ontouchstart' in window
      || navigator.maxTouchPoints > 0
      || window.matchMedia('(hover: none) and (pointer: coarse)').matches;
  }

  function render() {
    container.innerHTML = '';
    modifiers = { ctrl: false, alt: false, shift: false };
    activeKeys.forEach((key) => {
      const btn = document.createElement('button');
      btn.className = 'key-btn';
      btn.textContent = key.label;
      if (key.display && key.display !== key.label) {
        btn.setAttribute('aria-label', key.display);
      }

      const isModifier = key.type === 'modifier' || key.btn_type === 'modifier';

      const isAction = key.type === 'action' || key.btn_type === 'action';

      if (isModifier) {
        const modKey = key.mod || key.mod_key;
        btn.classList.add('modifier');
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          modifiers[modKey] = !modifiers[modKey];
          btn.classList.toggle('active', modifiers[modKey]);
        });
      } else if (isAction) {
        const actionName = key.action || key.btn_action;
        if (!actionName) return; // F013: skip invalid action buttons
        btn.classList.add('action');
        btn.dataset.action = actionName;
        btn.addEventListener('click', async (e) => {
          e.preventDefault();
          if (actionName === 'paste') {
            try {
              const text = await navigator.clipboard.readText();
              if (text) {
                const t = DenTerminal.getTerminal();
                if (t) t.paste(text);
              }
            } catch (err) {
              console.warn('Paste error:', err); // F002
              if (typeof Toast !== 'undefined') Toast.error('Clipboard access denied');
            }
          } else if (actionName === 'copy') {
            try {
              const t = DenTerminal.getTerminal();
              if (t) {
                const sel = t.getSelection();
                if (sel) {
                  await navigator.clipboard.writeText(sel);
                  t.clearSelection();
                  if (typeof Toast !== 'undefined') Toast.success('Copied');
                }
              }
            } catch (err) {
              console.warn('Copy error:', err); // F002
              if (typeof Toast !== 'undefined') Toast.error('Clipboard access denied');
            }
          } else if (actionName === 'select') {
            if (DenTerminal.isSelectMode()) {
              DenTerminal.exitSelectMode();
            } else {
              btn.classList.add('active');
              DenTerminal.enterSelectMode(() => {
                btn.classList.remove('active');
              });
              return; // Don't refocus terminal — overlay needs taps
            }
          } else if (SCROLL_ACTIONS[actionName]) {
            try {
              const t = DenTerminal.getTerminal();
              if (t) {
                SCROLL_ACTIONS[actionName](t);
              } else {
                console.warn('Scroll action ignored: terminal not available');
              }
            } catch (err) {
              console.warn('Scroll error:', err);
            }
            return; // Don't refocus — avoids opening soft keyboard on touch devices
          } else if (actionName === 'copy-screen') {
            try {
              const t = DenTerminal.getTerminal();
              if (t) {
                const buf = t.buffer.active;
                const lines = [];
                const end = Math.min(buf.viewportY + t.rows, buf.length); // F007: clamp
                for (let i = buf.viewportY; i < end; i++) {
                  const line = buf.getLine(i);
                  if (line) lines.push(line.translateToString(true));
                }
                const text = lines.join('\n').trimEnd(); // F007: single-pass trim
                if (text) {
                  await navigator.clipboard.writeText(text);
                  if (typeof Toast !== 'undefined') Toast.success('Screen copied');
                } else {
                  if (typeof Toast !== 'undefined') Toast.info('Nothing to copy'); // F012
                }
              }
            } catch (err) {
              // F002: distinguish clipboard vs other errors
              if (typeof Toast !== 'undefined') {
                Toast.error(err?.name === 'NotAllowedError' ? 'Clipboard access denied' : 'Copy failed');
              }
              console.warn('Screen copy error:', err);
            }
          }
          DenTerminal.focus();
        });
      } else {
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          let data = key.send;

          // 修飾パラメータ計算 (xterm: 1=none, 2=Shift, 3=Alt, 5=Ctrl, etc.)
          const modParam = (modifiers.shift ? 1 : 0)
            + (modifiers.alt ? 2 : 0)
            + (modifiers.ctrl ? 4 : 0);

          if (modParam > 0 && data.length > 2 && data.startsWith('\x1b[')) {
            // CSI シーケンス: ESC[A → ESC[1;2A, ESC[2~ → ESC[2;2~
            data = addCsiModifier(data, modParam + 1);
          } else {
            // Shift 修飾（単一文字）
            if (modifiers.shift && data.length === 1) {
              data = data.toUpperCase();
            }

            // Ctrl 修飾（単一文字）
            if (modifiers.ctrl && data.length === 1) {
              const code = data.toUpperCase().charCodeAt(0);
              if (code >= 0x40 && code <= 0x5f) {
                data = String.fromCharCode(code - 0x40);
              }
            }

            // Alt 修飾
            if (modifiers.alt) {
              data = '\x1b' + data;
            }
          }

          DenTerminal.sendInput(data);
          DenTerminal.focus();

          // 修飾キーをリセット
          resetModifiers();
        });
      }

      container.appendChild(btn);
    });
  }

  /** CSI シーケンスに修飾パラメータを付加
   *  制約: 既にセミコロン付きパラメータを含む CSI（例: ESC[1;5A）には未対応。
   *  DEFAULT_KEYS の CSI は単一パラメータまたはパラメータなしのため現状問題なし。 */
  function addCsiModifier(seq, mod) {
    // ESC[X → ESC[1;modX  (例: ESC[A → ESC[1;2A)
    // ESC[n~ → ESC[n;mod~ (例: ESC[2~ → ESC[2;2~)
    const body = seq.slice(2); // CSI 以降
    const finalChar = body[body.length - 1];
    const params = body.slice(0, -1);
    if (params === '') {
      return '\x1b[1;' + mod + finalChar;
    }
    return '\x1b[' + params + ';' + mod + finalChar;
  }

  function resetModifiers() {
    modifiers.ctrl = false;
    modifiers.alt = false;
    modifiers.shift = false;
    container.querySelectorAll('.modifier').forEach((btn) => {
      btn.classList.remove('active');
    });
  }

  /** 現在の修飾キー状態を返す（外部参照用） */
  function getModifiers() {
    return modifiers;
  }

  return { init, reload, getDefaultKeys, isTouchDevice, getModifiers, resetModifiers };
})();
