// Den - タッチキーバーモジュール
const Keybar = (() => {
  let container = null;
  let modifiers = { ctrl: false, alt: false, shift: false };
  let activeKeys = [];
  let currentPopup = null;
  let saveTimer = null;

  /**
   * Clipboard fallback for non-secure contexts (HTTP over LAN).
   * navigator.clipboard requires Secure Context (HTTPS or localhost).
   */
  async function clipboardWrite(text) {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return;
    }
    // Fallback: temporary textarea + execCommand
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;left:-9999px;top:-9999px;opacity:0';
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand('copy');
    } finally {
      ta.remove();
    }
  }

  async function clipboardRead() {
    if (navigator.clipboard && window.isSecureContext) {
      return await navigator.clipboard.readText();
    }
    // Fallback: prompt modal
    const text = await Toast.prompt('Paste text:');
    return text;
  }

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
    { type: 'stack', items: [
        { label: 'C-c', send: '\x03', display: 'Ctrl+C' },
        { label: 'C-z', send: '\x1a', display: 'Ctrl+Z' },
      ], selected: 0 },
    { type: 'stack', items: [
        { label: 'C-d', send: '\x04', display: 'Ctrl+D' },
        { label: 'C-l', send: '\x0c', display: 'Ctrl+L' },
      ], selected: 0 },
    { label: 'Sc\u2191', send: '', type: 'action', action: 'scroll-page-up', display: 'Scroll page up' },
    { label: 'Sc\u2193', send: '', type: 'action', action: 'scroll-page-down', display: 'Scroll page down' },
    { type: 'stack', items: [
        { label: 'Top', send: '', type: 'action', action: 'scroll-top', display: 'Scroll to top' },
        { label: 'Bot', send: '', type: 'action', action: 'scroll-bottom', display: 'Scroll to bottom' },
      ], selected: 0 },
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
    return DEFAULT_KEYS.map(k => {
      const copy = { ...k };
      if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
      return copy;
    });
  }

  function isTouchDevice() {
    return 'ontouchstart' in window
      || navigator.maxTouchPoints > 0
      || window.matchMedia('(hover: none) and (pointer: coarse)').matches;
  }

  /** アクション実行（paste/copy/select/scroll/copy-screen） */
  async function executeAction(actionName, btnEl) {
    if (actionName === 'paste') {
      try {
        const text = await clipboardRead();
        if (text) {
          const t = DenTerminal.getTerminal();
          if (t) t.paste(text);
        }
      } catch (err) {
        console.warn('Paste error:', err);
        if (typeof Toast !== 'undefined') Toast.error('Clipboard access denied');
      }
    } else if (actionName === 'copy') {
      try {
        const t = DenTerminal.getTerminal();
        if (t) {
          const sel = t.getSelection();
          if (sel) {
            await clipboardWrite(sel);
            t.clearSelection();
            if (typeof Toast !== 'undefined') Toast.success('Copied');
          }
        }
      } catch (err) {
        console.warn('Copy error:', err);
        if (typeof Toast !== 'undefined') Toast.error('Clipboard access denied');
      }
    } else if (actionName === 'select') {
      if (DenTerminal.isSelectMode()) {
        DenTerminal.exitSelectMode();
      } else {
        if (btnEl) btnEl.classList.add('active');
        DenTerminal.enterSelectMode(() => {
          if (btnEl) btnEl.classList.remove('active');
        });
        return 'no-focus'; // Don't refocus terminal — overlay needs taps
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
      return 'no-focus'; // Don't refocus — avoids opening soft keyboard on touch devices
    } else if (actionName === 'copy-screen') {
      try {
        const t = DenTerminal.getTerminal();
        if (t) {
          const buf = t.buffer.active;
          const lines = [];
          const end = Math.min(buf.viewportY + t.rows, buf.length);
          for (let i = buf.viewportY; i < end; i++) {
            const line = buf.getLine(i);
            if (line) lines.push(line.translateToString(true));
          }
          const text = lines.join('\n').trimEnd();
          if (text) {
            await clipboardWrite(text);
            if (typeof Toast !== 'undefined') Toast.success('Screen copied');
          } else {
            if (typeof Toast !== 'undefined') Toast.info('Nothing to copy');
          }
        }
      } catch (err) {
        if (typeof Toast !== 'undefined') {
          Toast.error(err?.name === 'NotAllowedError' ? 'Clipboard access denied' : 'Copy failed');
        }
        console.warn('Screen copy error:', err);
      }
    }
    DenTerminal.focus();
    return 'ok';
  }

  /** 通常キー送信（修飾キー適用） */
  function executeNormalKey(key) {
    let data = key.send;

    // 修飾パラメータ計算 (xterm: 1=none, 2=Shift, 3=Alt, 5=Ctrl, etc.)
    const modParam = (modifiers.shift ? 1 : 0)
      + (modifiers.alt ? 2 : 0)
      + (modifiers.ctrl ? 4 : 0);

    if (modParam > 0 && data.length > 2 && data.startsWith('\x1b[')) {
      data = addCsiModifier(data, modParam + 1);
    } else {
      if (modifiers.shift && data.length === 1) {
        data = data.toUpperCase();
      }
      if (modifiers.ctrl && data.length === 1) {
        const code = data.toUpperCase().charCodeAt(0);
        if (code >= 0x40 && code <= 0x5f) {
          data = String.fromCharCode(code - 0x40);
        }
      }
      if (modifiers.alt) {
        data = '\x1b' + data;
      }
    }

    DenTerminal.sendInput(data);
    DenTerminal.focus();
    resetModifiers();
  }

  /** 選択中アイテムを実行 */
  async function executeStackItem(item, btnEl) {
    const isAction = item.type === 'action' || item.btn_type === 'action';
    if (isAction) {
      const actionName = item.action || item.btn_action;
      if (actionName) await executeAction(actionName, btnEl);
    } else {
      executeNormalKey(item);
    }
  }

  /** スタックポップアップを開く */
  function openStackPopup(anchorBtn, stackKey, keyIndex) {
    closeStackPopup();

    const items = stackKey.items;
    const selectedIdx = stackKey.selected || 0;

    const popup = document.createElement('div');
    popup.className = 'stack-popup';
    popup.setAttribute('role', 'listbox');

    items.forEach((item, i) => {
      // modifier タイプはスタック内で使えないのでスキップ
      if (item.type === 'modifier' || item.btn_type === 'modifier') return;

      const opt = document.createElement('div');
      opt.className = 'stack-popup-item' + (i === selectedIdx ? ' selected' : '');
      opt.setAttribute('role', 'option');
      opt.setAttribute('aria-selected', i === selectedIdx ? 'true' : 'false');

      const check = document.createElement('span');
      check.className = 'stack-popup-check';
      check.textContent = i === selectedIdx ? '\u2713' : '';
      opt.appendChild(check);

      const label = document.createElement('span');
      label.textContent = item.display || item.label;
      opt.appendChild(label);

      opt.addEventListener('pointerdown', (e) => {
        e.preventDefault();
        e.stopPropagation();
        stackKey.selected = i;
        activeKeys[keyIndex] = stackKey;
        updateStackButton(anchorBtn, stackKey);
        closeStackPopup();
        scheduleSave();
      });

      popup.appendChild(opt);
    });

    // ポップアップ配置
    document.body.appendChild(popup);
    const anchorRect = anchorBtn.getBoundingClientRect();
    popup.style.bottom = (window.innerHeight - anchorRect.top + 4) + 'px';
    popup.style.left = anchorRect.left + 'px';

    // 画面端補正
    requestAnimationFrame(() => {
      const popupRect = popup.getBoundingClientRect();
      if (popupRect.right > window.innerWidth) {
        popup.style.left = Math.max(4, window.innerWidth - popupRect.width - 4) + 'px';
      }
      if (popupRect.left < 0) {
        popup.style.left = '4px';
      }
    });

    currentPopup = popup;
    anchorBtn.setAttribute('aria-expanded', 'true');

    // 外部タップで閉じる
    const onOutside = (e) => {
      if (!popup.contains(e.target) && e.target !== anchorBtn) {
        closeStackPopup();
      }
    };
    // 次のフレームでリスナー追加（開くイベントを拾わないように）
    requestAnimationFrame(() => {
      document.addEventListener('pointerdown', onOutside, { once: true, capture: true });
    });
    popup._outsideHandler = onOutside;
  }

  function closeStackPopup() {
    if (currentPopup) {
      if (currentPopup._outsideHandler) {
        document.removeEventListener('pointerdown', currentPopup._outsideHandler, { capture: true });
      }
      currentPopup.remove();
      // aria-expanded をリセット
      container.querySelectorAll('[aria-expanded="true"]').forEach(el => {
        el.setAttribute('aria-expanded', 'false');
      });
      currentPopup = null;
    }
  }

  /** スタックボタンの表示を更新 */
  function updateStackButton(btn, stackKey) {
    const items = stackKey.items;
    const sel = Math.min(stackKey.selected || 0, items.length - 1);
    const active = items[sel];
    const labelSpan = btn.querySelector('.stack-label');
    if (labelSpan) labelSpan.textContent = active.label;
    if (active.display && active.display !== active.label) {
      btn.setAttribute('aria-label', active.display);
    } else {
      btn.removeAttribute('aria-label');
    }
  }

  /** 選択変更を debounce で永続化 */
  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      if (typeof DenSettings !== 'undefined') {
        DenSettings.save({ keybar_buttons: activeKeys });
      }
    }, 2000);
  }

  function render() {
    container.innerHTML = '';
    closeStackPopup();
    modifiers = { ctrl: false, alt: false, shift: false };

    activeKeys.forEach((key, keyIndex) => {
      const isStack = key.type === 'stack' || key.btn_type === 'stack';
      const isModifier = key.type === 'modifier' || key.btn_type === 'modifier';
      const isAction = key.type === 'action' || key.btn_type === 'action';

      if (isStack) {
        const items = key.items;
        if (!items || items.length === 0) return; // 空スタックはスキップ
        const sel = Math.min(key.selected || 0, items.length - 1);
        key.selected = sel; // 正規化
        const active = items[sel];

        const btn = document.createElement('button');
        btn.className = 'key-btn stack';
        btn.setAttribute('aria-haspopup', 'listbox');
        btn.setAttribute('aria-expanded', 'false');
        if (active.display && active.display !== active.label) {
          btn.setAttribute('aria-label', active.display);
        }

        const labelSpan = document.createElement('span');
        labelSpan.className = 'stack-label';
        labelSpan.textContent = active.label;
        btn.appendChild(labelSpan);

        const indicator = document.createElement('span');
        indicator.className = 'stack-indicator';
        indicator.textContent = '\u25BC';
        indicator.setAttribute('aria-hidden', 'true');
        btn.appendChild(indicator);

        // 長押し検出用
        let pressTimer = null;
        let isLongPress = false;

        btn.addEventListener('pointerdown', (e) => {
          if (e.button !== 0) return;
          isLongPress = false;
          pressTimer = setTimeout(() => {
            isLongPress = true;
            openStackPopup(btn, key, keyIndex);
          }, 350);
        });

        btn.addEventListener('pointerup', (e) => {
          if (pressTimer) { clearTimeout(pressTimer); pressTimer = null; }
          if (!isLongPress) {
            e.preventDefault();
            executeStackItem(active, btn);
          }
        });

        btn.addEventListener('pointerleave', () => {
          if (pressTimer) { clearTimeout(pressTimer); pressTimer = null; }
        });

        btn.addEventListener('contextmenu', (e) => {
          e.preventDefault();
        });

        container.appendChild(btn);
        return;
      }

      const btn = document.createElement('button');
      btn.className = 'key-btn';
      btn.textContent = key.label;
      if (key.display && key.display !== key.label) {
        btn.setAttribute('aria-label', key.display);
      }

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
        if (!actionName) return;
        btn.classList.add('action');
        btn.dataset.action = actionName;
        btn.addEventListener('click', async (e) => {
          e.preventDefault();
          await executeAction(actionName, btn);
        });
      } else {
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          executeNormalKey(key);
        });
      }

      container.appendChild(btn);
    });
  }

  /** CSI シーケンスに修飾パラメータを付加
   *  制約: 既にセミコロン付きパラメータを含む CSI（例: ESC[1;5A）には未対応。
   *  DEFAULT_KEYS の CSI は単一パラメータまたはパラメータなしのため現状問題なし。 */
  function addCsiModifier(seq, mod) {
    const body = seq.slice(2);
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
