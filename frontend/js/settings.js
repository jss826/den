// Den - 設定管理モジュール
const DenSettings = (() => {
  let current = {
    font_size: 14,
    theme: 'dark',
    terminal_scrollback: 1000,
    claude_default_connection: null,
    claude_default_dir: null,
  };

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

  function openModal() {
    const modal = document.getElementById('settings-modal');
    document.getElementById('setting-font-size').value = current.font_size;
    document.getElementById('setting-scrollback').value = current.terminal_scrollback;
    document.getElementById('setting-default-dir').value = current.claude_default_dir || '';
    modal.hidden = false;
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

      await save({
        font_size: Math.max(8, Math.min(32, fontSize)),
        terminal_scrollback: Math.max(100, Math.min(50000, scrollback)),
        claude_default_dir: defaultDir,
      });
      apply();
      closeModal();
    });

    const modal = document.getElementById('settings-modal');
    if (modal) modal.addEventListener('click', (e) => {
      if (e.target === modal) closeModal();
    });
  }

  return { load, save, apply, get, getAll, bindUI, openModal };
})();
