// Den - タッチキーバーモジュール
const Keybar = (() => {
  let container = null;
  let modifiers = { ctrl: false, alt: false, shift: false };
  let activeKeys = [];

  // デフォルトキー配列
  const DEFAULT_KEYS = [
    { label: 'Ctrl', send: '', type: 'modifier', mod_key: 'ctrl' },
    { label: 'Alt', send: '', type: 'modifier', mod_key: 'alt' },
    { label: 'Shift', send: '', type: 'modifier', mod_key: 'shift' },
    { label: 'Tab', send: '\t' },
    { label: 'Esc', send: '\x1b' },
    { label: '\u2191', send: '\x1b[A' },
    { label: '\u2193', send: '\x1b[B' },
    { label: '\u2192', send: '\x1b[C' },
    { label: '\u2190', send: '\x1b[D' },
    { label: '|', send: '|' },
    { label: '~', send: '~' },
    { label: '/', send: '/' },
    { label: '-', send: '-' },
    { label: 'C-c', send: '\x03' },
    { label: 'C-d', send: '\x04' },
    { label: 'C-z', send: '\x1a' },
    { label: 'C-l', send: '\x0c' },
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

      const isModifier = key.type === 'modifier' || key.btn_type === 'modifier';

      if (isModifier) {
        const modKey = key.mod || key.mod_key;
        btn.classList.add('modifier');
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          modifiers[modKey] = !modifiers[modKey];
          btn.classList.toggle('active', modifiers[modKey]);
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
