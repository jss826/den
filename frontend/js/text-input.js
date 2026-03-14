/* global DenTerminal, Keybar */
// Den - Text Input Box module (mobile-friendly command input)
// eslint-disable-next-line no-unused-vars
const TextInput = (() => {
  const MAX_HISTORY = 50;
  const STORAGE_KEY = 'den_text_input_history';
  const VISIBLE_KEY = 'den_text_input_visible';

  let box = null;
  let textarea = null;
  let sendBtn = null;
  let historyBtn = null;
  let historyPopup = null;
  let historyRafId = null;

  function init() {
    box = document.getElementById('text-input-box');
    textarea = document.getElementById('text-input-textarea');
    sendBtn = document.getElementById('text-input-send');
    historyBtn = document.getElementById('text-input-history');
    if (!box || !textarea || !sendBtn) return;

    sendBtn.addEventListener('click', send);
    if (historyBtn) historyBtn.addEventListener('click', toggleHistory);

    textarea.addEventListener('keydown', (e) => {
      // Ctrl+Enter / Cmd+Enter: send
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
        e.preventDefault();
        send();
        return;
      }
      // Escape: focus terminal
      if (e.key === 'Escape') {
        e.preventDefault();
        DenTerminal.focus();
        return;
      }
    });

    // Restore visibility from localStorage
    if (localStorage.getItem(VISIBLE_KEY) === 'true') {
      show();
    }
  }

  function send() {
    const text = textarea.value;
    if (!text) return;

    // Convert \n to \r for PTY, process escape sequences
    let data = text.replace(/\n/g, '\r');
    data = Keybar.unescapeSend(data);

    DenTerminal.sendInput(data);
    addToHistory(text);
    textarea.value = '';
    textarea.focus();
  }

  function show() {
    if (!box) return;
    box.hidden = false;
    updateTerminalHeight();
    localStorage.setItem(VISIBLE_KEY, 'true');
  }

  function hide() {
    if (!box) return;
    box.hidden = true;
    closeHistory();
    updateTerminalHeight();
    localStorage.setItem(VISIBLE_KEY, 'false');
  }

  function toggle() {
    if (!box) return;
    if (box.hidden) {
      show();
      textarea.focus();
    } else {
      hide();
      DenTerminal.focus();
    }
  }

  function isVisible() {
    return box ? !box.hidden : false;
  }

  function focus() {
    if (textarea) textarea.focus();
  }

  /** Recalculate terminal container height when text input toggles */
  function updateTerminalHeight() {
    const container = document.getElementById('terminal-container');
    if (!container) return;
    if (box && !box.hidden) {
      container.style.height = `calc(100% - var(--terminal-session-bar-height) - ${box.offsetHeight}px)`;
    } else {
      container.style.height = '';
    }
    // Notify xterm.js to refit
    requestAnimationFrame(() => {
      DenTerminal.fitAndRefresh();
    });
  }

  // --- History ---

  function getHistory() {
    try {
      return JSON.parse(localStorage.getItem(STORAGE_KEY)) || [];
    } catch { return []; }
  }

  function addToHistory(text) {
    const trimmed = text.trim();
    if (!trimmed) return;
    let history = getHistory();
    // Remove duplicate if exists
    history = history.filter((h) => h !== trimmed);
    history.unshift(trimmed);
    if (history.length > MAX_HISTORY) history.length = MAX_HISTORY;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(history));
  }

  function toggleHistory() {
    if (historyPopup) {
      closeHistory();
    } else {
      openHistory();
    }
  }

  function openHistory() {
    closeHistory();
    const history = getHistory();

    historyPopup = document.createElement('div');
    historyPopup.className = 'text-input-history-popup';

    // Header
    const header = document.createElement('div');
    header.className = 'text-input-history-header';
    const title = document.createElement('span');
    title.textContent = 'Command History';
    header.appendChild(title);

    if (history.length > 0) {
      const clearBtn = document.createElement('button');
      clearBtn.className = 'text-input-history-clear';
      clearBtn.type = 'button';
      clearBtn.textContent = 'Clear';
      clearBtn.addEventListener('click', () => {
        localStorage.removeItem(STORAGE_KEY);
        closeHistory();
      });
      header.appendChild(clearBtn);
    }
    historyPopup.appendChild(header);

    if (history.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'text-input-history-empty';
      empty.textContent = 'No command history';
      historyPopup.appendChild(empty);
    } else {
      const list = document.createElement('div');
      list.className = 'text-input-history-list';

      for (const entry of history) {
        const item = document.createElement('button');
        item.className = 'text-input-history-item';
        item.type = 'button';

        const preview = document.createElement('span');
        preview.className = 'text-input-history-preview';
        preview.textContent = entry.length > 200 ? entry.slice(0, 200) + '...' : entry;
        item.appendChild(preview);

        item.addEventListener('click', () => {
          textarea.value = entry;
          textarea.focus();
          closeHistory();
        });

        list.appendChild(item);
      }
      historyPopup.appendChild(list);
    }

    document.body.appendChild(historyPopup);
    positionHistoryPopup();

    historyRafId = requestAnimationFrame(() => {
      if (!historyPopup) return;
      document.addEventListener('pointerdown', onHistoryOutsideClick, true);
      historyRafId = null;
    });
  }

  function positionHistoryPopup() {
    if (!historyBtn || !historyPopup) return;
    const rect = historyBtn.getBoundingClientRect();
    // Position above the button
    historyPopup.style.left = rect.left + 'px';
    historyPopup.style.bottom = (window.innerHeight - rect.top + 4) + 'px';
    // Clamp to viewport
    requestAnimationFrame(() => {
      if (!historyPopup) return;
      const popupRect = historyPopup.getBoundingClientRect();
      if (popupRect.right > window.innerWidth) {
        historyPopup.style.left = Math.max(4, window.innerWidth - popupRect.width - 4) + 'px';
      }
      if (popupRect.top < 4) {
        // Flip below if not enough space above
        historyPopup.style.bottom = '';
        historyPopup.style.top = (rect.bottom + 4) + 'px';
      }
    });
  }

  function closeHistory() {
    if (historyRafId !== null) {
      cancelAnimationFrame(historyRafId);
      historyRafId = null;
    }
    if (historyPopup) {
      historyPopup.remove();
      historyPopup = null;
    }
    document.removeEventListener('pointerdown', onHistoryOutsideClick, true);
  }

  function onHistoryOutsideClick(e) {
    if (historyPopup && !historyPopup.contains(e.target) && e.target !== historyBtn && !historyBtn.contains(e.target)) {
      closeHistory();
    }
  }

  function isHistoryOpen() {
    return historyPopup !== null;
  }

  return { init, toggle, isVisible, focus, closeHistory, isHistoryOpen };
})();
