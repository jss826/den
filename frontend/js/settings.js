// Den - 設定管理モジュール
const DenSettings = (() => {
  let current = {
    font_size: 14,
    theme: 'dark',
    terminal_scrollback: 1000,
    claude_default_connection: null,
    claude_default_dir: null,
    keybar_buttons: null,
  };

  // キーバー設定で使用する一時配列
  let editingKeybarButtons = null;

  // プリセットキー一覧 (label → send のマッピング)
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
  ];

  // エスケープ文字列をリテラルに変換
  function unescapeSend(str) {
    return str
      .replace(/\\x([0-9a-fA-F]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)))
      .replace(/\\t/g, '\t')
      .replace(/\\n/g, '\n')
      .replace(/\\r/g, '\r')
      .replace(/\\\\/g, '\\');
  }

  async function load() {
    try {
      const resp = await fetch('/api/settings', {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        current = await resp.json();
      }
    } catch (e) {
      console.warn('Failed to load settings:', e);
    }
    return current;
  }

  async function save(updates) {
    Object.assign(current, updates);
    try {
      await fetch('/api/settings', {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${Auth.getToken()}`,
        },
        body: JSON.stringify(current),
      });
    } catch (e) {
      console.warn('Failed to save settings:', e);
    }
  }

  function apply() {
    document.documentElement.style.setProperty('--den-font-size', current.font_size + 'px');
  }

  function get(key) {
    return current[key];
  }

  function getAll() {
    return { ...current };
  }

  // --- Keybar 設定 UI ---

  function renderKeybarList() {
    const list = document.getElementById('keybar-btn-list');
    if (!list) return;
    list.innerHTML = '';

    editingKeybarButtons.forEach((key, idx) => {
      const item = document.createElement('div');
      item.className = 'keybar-btn-item';
      if (key.type === 'modifier' || key.btn_type === 'modifier') {
        item.classList.add('modifier');
      }
      item.setAttribute('draggable', 'true');
      item.dataset.index = idx;

      const labelSpan = document.createElement('span');
      labelSpan.textContent = key.label;
      item.appendChild(labelSpan);

      const removeBtn = document.createElement('button');
      removeBtn.className = 'keybar-btn-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.type = 'button';
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        editingKeybarButtons.splice(idx, 1);
        renderKeybarList();
      });
      item.appendChild(removeBtn);

      // Desktop drag & drop
      item.addEventListener('dragstart', (e) => {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(idx));
        item.classList.add('dragging');
      });
      item.addEventListener('dragend', () => {
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
      });
      item.addEventListener('dragover', (e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        item.classList.add('drag-over');
      });
      item.addEventListener('dragleave', () => {
        item.classList.remove('drag-over');
      });
      item.addEventListener('drop', (e) => {
        e.preventDefault();
        item.classList.remove('drag-over');
        const fromIdx = parseInt(e.dataTransfer.getData('text/plain'), 10);
        const toIdx = idx;
        if (fromIdx !== toIdx) {
          const moved = editingKeybarButtons.splice(fromIdx, 1)[0];
          editingKeybarButtons.splice(toIdx, 0, moved);
          renderKeybarList();
        }
      });

      // Touch drag & drop
      let touchStartIdx = null;
      let touchClone = null;
      let touchCurrentOverIdx = null;

      item.addEventListener('touchstart', (e) => {
        if (e.target.classList.contains('keybar-btn-remove')) return;
        touchStartIdx = idx;
        const touch = e.touches[0];
        // Long press to initiate drag
        item._touchTimer = setTimeout(() => {
          item.classList.add('dragging');
          touchClone = item.cloneNode(true);
          touchClone.style.position = 'fixed';
          touchClone.style.zIndex = '999';
          touchClone.style.pointerEvents = 'none';
          touchClone.style.opacity = '0.8';
          touchClone.style.left = (touch.clientX - 20) + 'px';
          touchClone.style.top = (touch.clientY - 20) + 'px';
          document.body.appendChild(touchClone);
        }, 200);
      }, { passive: true });

      item.addEventListener('touchmove', (e) => {
        if (!touchClone && !item._touchTimer) return;
        if (!touchClone) return; // timer hasn't fired yet
        e.preventDefault();
        const touch = e.touches[0];
        touchClone.style.left = (touch.clientX - 20) + 'px';
        touchClone.style.top = (touch.clientY - 20) + 'px';

        // Find element under touch
        const overEl = document.elementFromPoint(touch.clientX, touch.clientY);
        const overItem = overEl ? overEl.closest('.keybar-btn-item') : null;
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (overItem && overItem.dataset.index !== undefined) {
          overItem.classList.add('drag-over');
          touchCurrentOverIdx = parseInt(overItem.dataset.index, 10);
        } else {
          touchCurrentOverIdx = null;
        }
      }, { passive: false });

      item.addEventListener('touchend', () => {
        clearTimeout(item._touchTimer);
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (touchClone) {
          touchClone.remove();
          touchClone = null;
          if (touchCurrentOverIdx !== null && touchStartIdx !== touchCurrentOverIdx) {
            const moved = editingKeybarButtons.splice(touchStartIdx, 1)[0];
            editingKeybarButtons.splice(touchCurrentOverIdx, 0, moved);
            renderKeybarList();
          }
        }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      });

      item.addEventListener('touchcancel', () => {
        clearTimeout(item._touchTimer);
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (touchClone) { touchClone.remove(); touchClone = null; }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      });

      list.appendChild(item);
    });
  }

  function getEditingButtons() {
    // 保存用: send 内のリテラル文字はそのまま保持
    return editingKeybarButtons.map(k => ({ ...k }));
  }

  function openModal() {
    const modal = document.getElementById('settings-modal');
    document.getElementById('setting-font-size').value = current.font_size;
    document.getElementById('setting-scrollback').value = current.terminal_scrollback;
    document.getElementById('setting-default-dir').value = current.claude_default_dir || '';

    // キーバー設定の初期化
    if (current.keybar_buttons && current.keybar_buttons.length > 0) {
      editingKeybarButtons = current.keybar_buttons.map(k => ({ ...k }));
    } else {
      editingKeybarButtons = Keybar.getDefaultKeys();
    }
    renderKeybarList();
    setupAddForm();

    // Add form を閉じた状態にリセット
    const addForm = document.getElementById('keybar-add-form');
    if (addForm) addForm.hidden = true;

    modal.hidden = false;
  }

  function setupAddForm() {
    const presetSelect = document.getElementById('keybar-preset-select');
    if (!presetSelect) return;
    // プリセット option を生成（初回のみ）
    if (presetSelect.options.length <= 1) {
      KEY_PRESETS.forEach(p => {
        const opt = document.createElement('option');
        opt.value = p.send;
        opt.textContent = p.display;
        opt.dataset.label = p.label;
        presetSelect.appendChild(opt);
      });
    }
  }

  function closeModal() {
    document.getElementById('settings-modal').hidden = true;
  }

  function bindUI() {
    const btn = document.getElementById('settings-btn');
    if (btn) btn.addEventListener('click', openModal);

    const cancelBtn = document.getElementById('settings-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', closeModal);

    const saveBtn = document.getElementById('settings-save');
    if (saveBtn) saveBtn.addEventListener('click', async () => {
      const fontSize = parseInt(document.getElementById('setting-font-size').value, 10) || 14;
      const scrollback = parseInt(document.getElementById('setting-scrollback').value, 10) || 1000;
      const defaultDir = document.getElementById('setting-default-dir').value.trim() || null;

      // キーバーボタン: 保存用に send をリテラルに変換
      const keybarButtons = getEditingButtons();

      await save({
        font_size: Math.max(8, Math.min(32, fontSize)),
        terminal_scrollback: Math.max(100, Math.min(50000, scrollback)),
        claude_default_dir: defaultDir,
        keybar_buttons: keybarButtons,
      });
      apply();

      // キーバーを即時反映
      Keybar.reload(keybarButtons);

      closeModal();
    });

    const modal = document.getElementById('settings-modal');
    if (modal) modal.addEventListener('click', (e) => {
      if (e.target === modal) closeModal();
    });

    // --- Keybar editor ---
    const addBtn = document.getElementById('keybar-add-btn');
    const resetBtn = document.getElementById('keybar-reset-btn');
    const addForm = document.getElementById('keybar-add-form');
    const addConfirm = document.getElementById('keybar-add-confirm');
    const addCancel = document.getElementById('keybar-add-cancel');
    const presetSelect = document.getElementById('keybar-preset-select');
    const newLabelInput = document.getElementById('keybar-new-label');
    const newSendInput = document.getElementById('keybar-new-send');
    const newModifierCheck = document.getElementById('keybar-new-modifier');
    const newModKeySelect = document.getElementById('keybar-new-modkey');

    if (addBtn) addBtn.addEventListener('click', () => {
      addForm.hidden = false;
      newLabelInput.value = '';
      newSendInput.value = '';
      presetSelect.value = '';
      newModifierCheck.checked = false;
      newModKeySelect.hidden = true;
      newLabelInput.focus();
    });

    if (resetBtn) resetBtn.addEventListener('click', () => {
      editingKeybarButtons = Keybar.getDefaultKeys();
      renderKeybarList();
    });

    if (presetSelect) presetSelect.addEventListener('change', () => {
      const val = presetSelect.value;
      if (val) {
        const opt = presetSelect.selectedOptions[0];
        newLabelInput.value = opt.dataset.label || '';
        newSendInput.value = val;
      } else {
        newSendInput.value = '';
      }
    });

    if (newModifierCheck) newModifierCheck.addEventListener('change', () => {
      newModKeySelect.hidden = !newModifierCheck.checked;
    });

    if (addConfirm) addConfirm.addEventListener('click', () => {
      const label = newLabelInput.value.trim();
      if (!label) {
        newLabelInput.focus();
        return;
      }

      if (newModifierCheck.checked) {
        editingKeybarButtons.push({
          label,
          send: '',
          type: 'modifier',
          mod_key: newModKeySelect.value,
        });
      } else {
        const sendRaw = newSendInput.value;
        if (!sendRaw) {
          newSendInput.focus();
          return;
        }
        const sendValue = unescapeSend(sendRaw);
        editingKeybarButtons.push({
          label,
          send: sendValue,
        });
      }

      renderKeybarList();
      addForm.hidden = true;
    });

    if (addCancel) addCancel.addEventListener('click', () => {
      addForm.hidden = true;
    });
  }

  return { load, save, apply, get, getAll, bindUI, openModal };
})();
