// Den - フローティングキーバーモジュール
const Keybar = (() => {
  let container = null;
  let buttonsContainer = null;
  let dragHandle = null;
  let collapseBtn = null;
  let tabEl = null;
  let modifiers = { ctrl: false, alt: false, shift: false };
  let activeKeys = [];
  let currentPopup = null;
  let saveTimer = null;

  // Floating state
  let collapsed = false;
  let keybarVisible = true;
  let collapseSide = 'right'; // "right" | "left"
  let dragState = null;
  let tabDragState = null;
  let positionSaveTimer = null;

  const SAVE_DEBOUNCE_MS = 2000;

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
    { label: 'Enter', send: '\r' },
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
    buttonsContainer = container.querySelector('.keybar-buttons');
    dragHandle = container.querySelector('.keybar-drag-handle');
    collapseBtn = container.querySelector('.keybar-collapse-btn');
    tabEl = document.getElementById('keybar-tab');

    activeKeys = customKeys && customKeys.length > 0 ? customKeys : DEFAULT_KEYS;
    render();

    // Drag handle
    dragHandle.addEventListener('pointerdown', onDragStart);

    // Collapse/expand
    collapseBtn.addEventListener('click', collapse);
    tabEl.addEventListener('click', onTabClick);
    tabEl.addEventListener('pointerdown', onTabDragStart);

    // Restore position from settings
    restorePosition();

    // Viewport resize → clamp (named function for potential removeEventListener)
    window.addEventListener('resize', onWindowResize);

    // Flush pending saves on page hide (tab switch, navigation, reload)
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') flushPendingSaves();
    });
  }

  // F014: Debounce resize with rAF to avoid high-frequency DOM writes
  let resizeRafId = null;
  function onWindowResize() {
    if (resizeRafId) return;
    resizeRafId = requestAnimationFrame(() => {
      resizeRafId = null;
      clampToViewport();
    });
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

  // F007: Guard against matchMedia unavailability
  const IS_TOUCH_DEVICE = 'ontouchstart' in window
    || navigator.maxTouchPoints > 0
    || (typeof window.matchMedia === 'function'
        && window.matchMedia('(hover: none) and (pointer: coarse)').matches);

  function isTouchDevice() {
    return IS_TOUCH_DEVICE;
  }

  // --- Visibility ---

  function applyVisibility() {
    if (keybarVisible) {
      if (collapsed) {
        container.hidden = true;
        tabEl.hidden = false;
      } else {
        container.hidden = false;
        tabEl.hidden = true;
      }
    } else {
      container.hidden = true;
      tabEl.hidden = true;
    }
  }

  function toggleVisibility() {
    if (!container) return;
    const wasHidden = !keybarVisible;
    keybarVisible = !keybarVisible;
    applyVisibility();
    if (wasHidden && keybarVisible) {
      requestAnimationFrame(() => clampToViewport());
    }
    schedulePositionSave();
  }

  function isVisible() {
    return keybarVisible;
  }

  // --- Collapse / Expand ---

  function collapse() {
    if (collapsed) return;
    collapsed = true;
    const barRect = container.getBoundingClientRect();
    // Place tab at bar's Y position, snapped to collapseSide edge
    tabEl.dataset.side = collapseSide;
    tabEl.style.removeProperty('left');
    tabEl.style.removeProperty('right');
    tabEl.style.top = barRect.top + 'px';
    applyVisibility();
    schedulePositionSave();
  }

  function expand() {
    if (!collapsed) return;
    collapsed = false;
    const tabRect = tabEl.getBoundingClientRect();
    // Place bar at tab's Y position (X is always full-width)
    container.style.top = tabRect.top + 'px';
    applyVisibility();
    // After showing, clamp to viewport
    requestAnimationFrame(() => clampToViewport());
    schedulePositionSave();
  }

  // --- Drag (keybar) ---

  function onDragStart(e) {
    if (e.button !== 0) return;
    e.preventDefault();
    const rect = container.getBoundingClientRect();
    dragState = {
      startY: e.clientY,
      origTop: rect.top,
      vh: window.innerHeight,
    };
    dragHandle.setPointerCapture(e.pointerId);
    document.addEventListener('pointermove', onDragMove);
    document.addEventListener('pointerup', onDragEnd);
    document.addEventListener('pointercancel', onDragEnd);
  }

  function onDragMove(e) {
    if (!dragState) return;
    const dy = e.clientY - dragState.startY;
    let newTop = dragState.origTop + dy;

    newTop = Math.max(0, Math.min(newTop, dragState.vh - 40));

    container.style.top = newTop + 'px';
  }

  function onDragEnd() {
    dragState = null;
    document.removeEventListener('pointermove', onDragMove);
    document.removeEventListener('pointerup', onDragEnd);
    document.removeEventListener('pointercancel', onDragEnd);
    schedulePositionSave();
  }

  // --- Drag (tab) ---

  const TAB_DRAG_THRESHOLD = 3;
  let lastTabWasDrag = false;

  function onTabDragStart(e) {
    if (e.button !== 0) return;
    e.preventDefault();
    const rect = tabEl.getBoundingClientRect();
    tabDragState = {
      startX: e.clientX,
      startY: e.clientY,
      origTop: rect.top,
      dist: 0,
      lastPointerX: e.clientX,
      vh: window.innerHeight,
      vw: window.innerWidth,
    };
    tabEl.setPointerCapture(e.pointerId);
    document.addEventListener('pointermove', onTabDragMove);
    document.addEventListener('pointerup', onTabDragEnd);
    document.addEventListener('pointercancel', onTabDragEnd);
  }

  function onTabDragMove(e) {
    if (!tabDragState) return;
    const dx = e.clientX - tabDragState.startX;
    const dy = e.clientY - tabDragState.startY;
    tabDragState.dist = Math.max(tabDragState.dist, Math.abs(dx) + Math.abs(dy));
    tabDragState.lastPointerX = e.clientX;

    if (tabDragState.dist < TAB_DRAG_THRESHOLD) return; // Not a drag yet

    // Y-axis movement only
    let newTop = tabDragState.origTop + dy;
    newTop = Math.max(0, Math.min(newTop, tabDragState.vh - 44));
    tabEl.style.top = newTop + 'px';

    // Live preview: snap side based on pointer X position
    const newSide = e.clientX < tabDragState.vw / 2 ? 'left' : 'right';
    if (tabEl.dataset.side !== newSide) {
      tabEl.dataset.side = newSide;
      tabEl.style.removeProperty('left');
      tabEl.style.removeProperty('right');
    }
  }

  function onTabDragEnd() {
    const wasDrag = tabDragState && tabDragState.dist >= TAB_DRAG_THRESHOLD;
    if (wasDrag) {
      // Snap collapseSide based on final pointer X position
      collapseSide = tabDragState.lastPointerX < tabDragState.vw / 2 ? 'left' : 'right';
      tabEl.dataset.side = collapseSide;
      tabEl.style.removeProperty('left');
      tabEl.style.removeProperty('right');
    }
    lastTabWasDrag = wasDrag;
    tabDragState = null;
    document.removeEventListener('pointermove', onTabDragMove);
    document.removeEventListener('pointerup', onTabDragEnd);
    document.removeEventListener('pointercancel', onTabDragEnd);
    if (wasDrag) schedulePositionSave();
  }

  function onTabClick() {
    if (lastTabWasDrag) return; // Was a drag, not a tap
    expand();
  }

  // --- Position persistence ---

  /** Immediately flush any pending debounced saves (position + stack selection).
   *  Called on visibilitychange→hidden to prevent data loss on page unload. */
  function flushPendingSaves() {
    if (typeof DenSettings === 'undefined') return;
    let updates = null;
    if (positionSaveTimer) {
      clearTimeout(positionSaveTimer);
      positionSaveTimer = null;
      updates = { keybar_position: getCurrentPosition() };
    }
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
      updates = updates || {};
      updates.keybar_buttons = activeKeys;
    }
    if (updates) {
      DenSettings.save(updates, { keepalive: true }).catch(() => {});
    }
  }

  function schedulePositionSave() {
    if (positionSaveTimer) clearTimeout(positionSaveTimer);
    positionSaveTimer = setTimeout(async () => {
      positionSaveTimer = null;
      if (typeof DenSettings === 'undefined') return;
      const pos = getCurrentPosition();
      // F002: Catch save errors to prevent silent failures
      try {
        await DenSettings.save({ keybar_position: pos });
      } catch (err) {
        console.warn('[Keybar] Failed to save position:', err);
      }
    }, SAVE_DEBOUNCE_MS);
  }

  function getCurrentPosition() {
    // Use style.top instead of getBoundingClientRect() — hidden elements
    // (display:none via hidden attribute) return {top:0} from getBoundingClientRect(),
    // which corrupts the saved position when keybar is hidden via Ctrl+K toggle.
    const el = collapsed ? tabEl : container;
    const top = parseFloat(el.style.top);
    return {
      left: 0,
      top: Number.isFinite(top) ? top : 0,
      visible: keybarVisible,
      collapsed: collapsed,
      collapse_side: collapseSide,
    };
  }

  function restorePosition() {
    if (typeof DenSettings === 'undefined') return;
    const pos = DenSettings.get('keybar_position');

    if (pos && Number.isFinite(pos.top)) {
      keybarVisible = pos.visible !== false;
      collapsed = !!pos.collapsed;
      collapseSide = (pos.collapse_side === 'left' || pos.collapse_side === 'right')
        ? pos.collapse_side : 'right';

      container.style.top = pos.top + 'px';
      tabEl.style.top = pos.top + 'px';
      tabEl.dataset.side = collapseSide;
      tabEl.style.removeProperty('left');
      tabEl.style.removeProperty('right');
    } else {
      // Default: bottom
      setDefaultPosition();
    }

    applyVisibility();
    requestAnimationFrame(() => clampToViewport());
  }

  function setDefaultPosition() {
    // Position at bottom after DOM layout
    requestAnimationFrame(() => {
      const vh = window.innerHeight;
      const top = vh - 60;
      container.style.top = top + 'px';
      tabEl.style.top = top + 'px';
      tabEl.dataset.side = collapseSide;
      tabEl.style.removeProperty('left');
      tabEl.style.removeProperty('right');
    });
  }

  function clampToViewport() {
    if (!keybarVisible) return;
    const vh = window.innerHeight;

    // Read phase
    const barVisible = !container.hidden;
    const tabVisible = !tabEl.hidden;
    const barRect = barVisible ? container.getBoundingClientRect() : null;
    const tabRect = tabVisible ? tabEl.getBoundingClientRect() : null;

    // Write phase — bar (Y-axis only, full-width)
    if (barRect) {
      const top = Math.max(0, Math.min(barRect.top, vh - 40));
      container.style.top = top + 'px';
    }

    // Write phase — tab (Y-axis only, side handled by CSS)
    if (tabRect) {
      const top = Math.max(0, Math.min(tabRect.top, vh - 44));
      tabEl.style.top = top + 'px';
    }
  }

  // --- Actions ---

  /** アクション実行（paste/copy/select/scroll/copy-screen） */
  async function executeAction(actionName, btnEl) {
    if (actionName === 'paste') {
      try {
        const text = await DenClipboard.read();
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
            await DenClipboard.write(sel);
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
            await DenClipboard.write(text);
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

  /** エスケープ文字列をリテラルに変換（設定由来の \\r \\t \\xNN 等を修復） */
  function unescapeSend(str) {
    return str.replace(/\\(x([0-9a-fA-F]{2})|t|n|r|\\)/g, (_, p1, hex) => {
      if (hex) return String.fromCharCode(parseInt(hex, 16));
      if (p1 === 't') return '\t';
      if (p1 === 'n') return '\n';
      if (p1 === 'r') return '\r';
      if (p1 === '\\') return '\\';
      return _;
    });
  }

  /** 通常キー送信（修飾キー適用） */
  function executeNormalKey(key) {
    let data = unescapeSend(key.send);

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

    // ポップアップ配置 — バーが上半分にあれば下に、下半分にあれば上に
    document.body.appendChild(popup);
    const anchorRect = anchorBtn.getBoundingClientRect();
    const barRect = container.getBoundingClientRect();
    const spaceBelow = window.innerHeight - barRect.bottom;
    const spaceAbove = barRect.top;

    if (spaceBelow > spaceAbove || spaceAbove < 100) {
      // Show below
      popup.style.top = (anchorRect.bottom + 4) + 'px';
      popup.style.bottom = '';
    } else {
      // Show above
      popup.style.bottom = (window.innerHeight - anchorRect.top + 4) + 'px';
      popup.style.top = '';
    }
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
      const target = buttonsContainer || container;
      target.querySelectorAll('[aria-expanded="true"]').forEach(el => {
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
    }, SAVE_DEBOUNCE_MS);
  }

  function render() {
    const target = buttonsContainer || container;
    target.innerHTML = '';
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

        target.appendChild(btn);
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

      target.appendChild(btn);
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
    const target = buttonsContainer || container;
    target.querySelectorAll('.modifier').forEach((btn) => {
      btn.classList.remove('active');
    });
  }

  /** 現在の修飾キー状態を返す（外部参照用） */
  function getModifiers() {
    return modifiers;
  }

  return {
    init, reload, getDefaultKeys, isTouchDevice,
    getModifiers, resetModifiers, executeKey: executeNormalKey,
    toggleVisibility, collapse, expand, isVisible,
  };
})();
