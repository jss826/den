/* global DenClipboard, Toast */
// Den - クリップボード履歴モジュール
// eslint-disable-next-line no-unused-vars
const ClipboardHistory = (() => {
  let btn = null;
  let popup = null;
  let rafId = null;
  let resizeRaf = null;

  function init(button) {
    btn = button;
    if (!btn) return;
    btn.addEventListener('click', toggle);
  }

  function toggle() {
    if (popup) {
      close();
    } else {
      open();
    }
  }

  async function open() {
    close();

    let entries;
    try {
      const resp = await fetch('/api/clipboard-history', { credentials: 'same-origin' });
      if (!resp.ok) { Toast.error('Failed to load clipboard history'); return; }
      entries = await resp.json();
    } catch {
      Toast.error('Failed to load clipboard history');
      return;
    }

    popup = document.createElement('div');
    popup.className = 'clipboard-history-popup';

    // Header
    const header = document.createElement('div');
    header.className = 'clipboard-history-header';
    const title = document.createElement('span');
    title.textContent = 'Clipboard History';
    header.appendChild(title);

    if (entries.length > 0) {
      const clearBtn = document.createElement('button');
      clearBtn.className = 'clipboard-history-clear';
      clearBtn.type = 'button';
      clearBtn.textContent = 'Clear';
      clearBtn.addEventListener('click', async () => {
        try {
          const resp = await fetch('/api/clipboard-history', { method: 'DELETE', credentials: 'same-origin' });
          if (!resp.ok) { Toast.error('Failed to clear history'); return; }
          close();
          Toast.success('History cleared');
        } catch {
          Toast.error('Failed to clear history');
        }
      });
      header.appendChild(clearBtn);
    }
    popup.appendChild(header);

    if (entries.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'clipboard-history-empty';
      empty.textContent = 'No clipboard history';
      popup.appendChild(empty);
    } else {
      const list = document.createElement('div');
      list.className = 'clipboard-history-list';

      for (const entry of entries) {
        const item = document.createElement('button');
        item.className = 'clipboard-history-item';
        item.type = 'button';

        const preview = document.createElement('span');
        preview.className = 'clipboard-history-preview';
        preview.textContent = entry.text.length > 200 ? entry.text.slice(0, 200) + '...' : entry.text;
        item.appendChild(preview);

        const meta = document.createElement('span');
        meta.className = 'clipboard-history-meta';

        const time = document.createElement('span');
        time.className = 'clipboard-history-time';
        time.textContent = formatTime(entry.timestamp);
        meta.appendChild(time);

        if (entry.source !== 'copy') {
          const badge = document.createElement('span');
          badge.className = 'clipboard-history-badge';
          badge.textContent = entry.source === 'osc52' ? 'OSC52' : 'System';
          meta.appendChild(badge);
        }

        item.appendChild(meta);

        item.addEventListener('click', async () => {
          try {
            await DenClipboard.write(entry.text, { skipTrack: true });
            Toast.success('Copied');
          } catch {
            Toast.error('Copy failed');
          }
          close();
        });

        list.appendChild(item);
      }
      popup.appendChild(list);
    }

    document.body.appendChild(popup);
    positionPopup();

    window.addEventListener('resize', onResize);
    rafId = requestAnimationFrame(() => {
      if (!popup) return;
      document.addEventListener('pointerdown', onOutsideClick, true);
      rafId = null;
    });
  }

  function onResize() {
    if (resizeRaf) return;
    resizeRaf = requestAnimationFrame(() => { resizeRaf = null; positionPopup(); });
  }

  function positionPopup() {
    if (!btn || !popup) return;
    const rect = btn.getBoundingClientRect();
    popup.style.top = (rect.bottom + 4) + 'px';
    popup.style.right = (window.innerWidth - rect.right) + 'px';
  }

  function close() {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    if (popup) {
      popup.remove();
      popup = null;
    }
    if (resizeRaf !== null) {
      cancelAnimationFrame(resizeRaf);
      resizeRaf = null;
    }
    window.removeEventListener('resize', onResize);
    document.removeEventListener('pointerdown', onOutsideClick, true);
  }

  function onOutsideClick(e) {
    if (popup && !popup.contains(e.target) && e.target !== btn && !btn.contains(e.target)) {
      close();
    }
  }

  function isOpen() {
    return popup !== null;
  }

  /** Fire-and-forget POST to track a clipboard entry */
  function track(text, source) {
    if (!text) return;
    fetch('/api/clipboard-history', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, source: source || 'copy' }),
    }).catch(() => { /* ignore tracking errors */ });
  }

  function formatTime(ts) {
    const d = new Date(ts);
    const now = new Date();
    const diff = now - d;
    if (diff < 60000) return 'just now';
    if (diff < 3600000) return Math.floor(diff / 60000) + 'm ago';
    if (diff < 86400000) return Math.floor(diff / 3600000) + 'h ago';
    return d.toLocaleDateString();
  }

  return { init, open, close, isOpen, track };
})();
